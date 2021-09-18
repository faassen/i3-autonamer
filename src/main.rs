use anyhow::{Error, Result};
use std;
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_i3ipc::{
    event::{Event, Subscribe, WindowChange, WorkspaceChange},
    reply,
    reply::{Node, NodeType},
    I3,
};
use tokio_stream::StreamExt;

type Lookup = HashMap<String, String>;

fn get_leaf_content_nodes<'a>(node: &'a Node) -> Vec<&Node> {
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

fn get_nodes_of_type<'a>(node: &'a Node, node_type: NodeType) -> impl Iterator<Item = &'a Node> {
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

async fn get_workspace_rename_commands(tree: &Node) -> Result<Vec<String>> {
    let mut lookup: Lookup = HashMap::new();
    lookup.insert("Alacritty".to_string(), 'A'.to_string());
    lookup.insert("Joplin".to_string(), 'J'.to_string());
    lookup.insert("Firefox".to_string(), 'F'.to_string());
    lookup.insert("Code".to_string(), 'C'.to_string());
    let workspace_nodes = get_workspace_nodes(&tree);
    // log::debug!("root: {:#?}", root);
    Ok(workspace_nodes
        .filter_map(|workspace_node| {
            let num = workspace_node.num?;
            if num < 1 {
                return None;
            }
            let name = get_workspace_name(workspace_node, &lookup);
            let full_name = if name.len() > 0 {
                format!("{}: {}", num, name)
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

fn update_workspace_names(tx: &mpsc::Sender<Command>) -> JoinHandle<Result<()>> {
    let tx2 = tx.clone();
    return tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::GetTree { resp: resp_tx };
        tx2.send(cmd).await?;
        log::debug!("Waiting for tree");
        let tree = resp_rx.await?;
        log::debug!("We got tree!");
        log::debug!("{:?}", tree);
        let commands = get_workspace_rename_commands(&tree?).await?;
        for command in commands {
            // log::debug!("Command: {}", command);
            let tx3 = tx2.clone();
            tokio::spawn(async move {
                let (resp_tx, resp_rx) = oneshot::channel();
                let cmd = Command::RunCommand {
                    payload: command,
                    resp: resp_tx,
                };
                if tx3.send(cmd).await.is_err() {
                    log::debug!("Error when moving");
                    return;
                };
                let _ = resp_rx.await;
            })
            .await?;
        }
        return Ok(());
    });
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    flexi_logger::Logger::try_with_env()?.start()?;
    let (tx, mut rx) = mpsc::channel::<Command>(10);

    let s_handle = tokio::spawn(async move {
        let mut event_listener = {
            let mut i3 = I3::connect().await?;
            i3.subscribe([Subscribe::Window, Subscribe::Workspace])
                .await?;
            i3.listen()
        };

        while let Some(event) = event_listener.next().await {
            match event? {
                Event::Window(window_data) => {
                    match window_data.change {
                        WindowChange::New
                        | WindowChange::Close
                        | WindowChange::Move
                        | WindowChange::Floating => {
                            log::debug!("WindowChange");
                            // new, close, move, floating (?)
                            update_workspace_names(&tx).await;
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
                            update_workspace_names(&tx).await;
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
                    log::debug!("RunCommand {}", payload);
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
