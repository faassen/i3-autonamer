use anyhow::{Error, Result};
use serde_derive::Deserialize;
use std;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_i3ipc::{
    event::{Event, Subscribe, WindowChange, WorkspaceChange},
    reply,
    reply::{Node, NodeType},
    I3,
};
use tokio_stream::StreamExt;
use toml;

type Lookup = Arc<Mutex<HashMap<String, String>>>;

fn get_leaf_content_nodes(node: &Node) -> Vec<&Node> {
    get_nodes_of_type(node, NodeType::Con)
        .flat_map(|n| {
            if n.nodes.len() == 0 {
                vec![n]
            } else {
                get_leaf_content_nodes(n)
            }
        })
        .collect()
}

fn get_workspace_name(workspace_node: &Node, lookup: &Lookup) -> String {
    let lookup = lookup.lock().unwrap();
    let names = get_leaf_content_nodes(workspace_node)
        .iter()
        .filter_map(|n| {
            let class_name = (n.window_properties).as_ref()?.class.as_ref()?;
            log::debug!("class__name: {}", class_name);
            lookup.get(class_name)
        })
        .cloned()
        .collect::<HashSet<_>>();
    names.into_iter().collect::<Vec<_>>().join(" ")
}

fn get_nodes_of_type(node: &Node, node_type: NodeType) -> impl Iterator<Item = &Node> {
    node.nodes.iter().filter(move |n| n.node_type == node_type)
}

fn get_workspace_nodes(root: &Node) -> impl Iterator<Item = &Node> {
    assert!(root.node_type == NodeType::Root);
    get_nodes_of_type(&root, NodeType::Output)
        .map(|n| get_nodes_of_type(n, NodeType::Con))
        .flatten()
        .map(|n| get_nodes_of_type(n, NodeType::Workspace))
        .flatten()
}

#[derive(Deserialize, Debug)]
struct Config {
    window_class: HashMap<String, String>,
}

async fn get_workspace_rename_commands(tree: &Node, lookup: Lookup) -> Result<Vec<String>> {
    let workspace_nodes = get_workspace_nodes(&tree);
    Ok(workspace_nodes
        .filter_map(|workspace_node| {
            let num = workspace_node.num?;
            if num < 1 {
                return None;
            }
            let name = get_workspace_name(workspace_node, &lookup);
            let full_name = if name.len() > 0 {
                format!("{} {}", num, name)
            } else {
                format!("{}", num)
            };
            log::debug!("Workspace num {} to {}", num, full_name);
            Some(format!(
                "rename workspace \"{}\" to \"{}\"",
                workspace_node.name.clone().unwrap(),
                full_name
            ))
        })
        .collect())
}

type Responder<T> = oneshot::Sender<anyhow::Result<T, std::io::Error>>;

#[derive(Debug)]
enum Command {
    RunCommand {
        payload: String,
        resp: Responder<Vec<reply::Success>>,
    },
    GetTree {
        resp: Responder<Node>,
    },
}

fn spawn_update_workspace_names(
    tx: &mpsc::Sender<Command>,
    lookup: Lookup,
) -> JoinHandle<Result<()>> {
    let tx2 = tx.clone();
    return tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::GetTree { resp: resp_tx };
        tx2.send(cmd).await?;
        let tree = resp_rx.await?;
        let commands = get_workspace_rename_commands(&tree?, lookup).await?;
        for command in commands {
            // log::debug!("Command: {}", command);
            spawn_command(&tx2, command).await??;
        }
        return Ok(());
    });
}

fn spawn_command(tx: &mpsc::Sender<Command>, payload: String) -> JoinHandle<Result<()>> {
    let tx2 = tx.clone();
    return tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::RunCommand {
            payload: payload,
            resp: resp_tx,
        };
        tx2.send(cmd).await?;
        let _ = resp_rx.await;
        return Ok(());
    });
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    flexi_logger::Logger::try_with_env()?.start()?;
    let (tx, mut rx) = mpsc::channel::<Command>(10);

    let config: Config = toml::from_str(&fs::read_to_string("config-example.toml".to_string())?)?;
    let lookup = Arc::new(Mutex::new(config.window_class));

    let s_handle = tokio::spawn(async move {
        let mut event_listener = {
            let mut i3 = I3::connect().await?;
            i3.subscribe([Subscribe::Window, Subscribe::Workspace])
                .await?;
            i3.listen()
        };

        while let Some(event) = event_listener.next().await {
            let lookup = lookup.clone();
            match event? {
                Event::Window(window_data) => {
                    match window_data.change {
                        WindowChange::New
                        | WindowChange::Close
                        | WindowChange::Move
                        | WindowChange::Floating => {
                            log::debug!("WindowChange");
                            // new, close, move, floating (?)
                            spawn_update_workspace_names(&tx, lookup).await??;
                        }
                        _ => {}
                    }
                }
                Event::Workspace(workspace_data) => {
                    // init
                    // empty
                    // reload
                    // rename
                    // restored ?
                    // move
                    match workspace_data.change {
                        WorkspaceChange::Init | WorkspaceChange::Empty | WorkspaceChange::Move => {
                            log::debug!("WorkspaceChange");
                            spawn_update_workspace_names(&tx, lookup).await??;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        log::debug!("Sender loop ended");
        Ok::<_, Error>(())
    });

    let r_handle = tokio::spawn(async move {
        let mut i3 = I3::connect().await?;
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::RunCommand { payload, resp } => {
                    let res = i3.run_command(payload).await;
                    let _ = resp.send(res);
                }
                Command::GetTree { resp } => {
                    let res = i3.get_tree().await;
                    let _ = resp.send(res);
                }
            }
        }
        log::debug!("Receiver loop ended");
        Ok::<_, Error>(())
    });

    let (send, recv) = tokio::try_join!(s_handle, r_handle)?;
    send.and(recv)?;
    Ok(())
}
