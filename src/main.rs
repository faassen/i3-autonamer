use anyhow::{Error, Result};
use std;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio_i3ipc::{
    event::{Event, Subscribe, WindowChange, WorkspaceChange},
    msg::Msg,
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

async fn get_workspace_rename_commands(i3: &mut I3) -> Result<Vec<String>> {
    let mut lookup: Lookup = HashMap::new();
    lookup.insert("Alacritty".to_string(), 'A'.to_string());
    lookup.insert("Joplin".to_string(), 'J'.to_string());
    lookup.insert("Firefox".to_string(), 'F'.to_string());
    lookup.insert("Code".to_string(), 'C'.to_string());
    let root = i3.get_tree().await?;
    let workspace_nodes = get_workspace_nodes(&root);
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

async fn update_workspace_names(i3: &mut I3, send: &mpsc::Sender<String>) -> Result<()> {
    let commands = get_workspace_rename_commands(i3).await?;
    for command in commands {
        log::debug!("Command: {}", command);
        send.send(command).await?;
    }
    return Ok(());
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    flexi_logger::Logger::try_with_env()?.start()?;
    let (send, mut recv) = mpsc::channel(10);

    let s_handle = tokio::spawn(async move {
        let mut event_listener = {
            let mut i3 = I3::connect().await?;
            i3.subscribe([Subscribe::Window, Subscribe::Workspace])
                .await?;
            i3.listen()
        };

        let i3 = &mut I3::connect().await?;

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
                            update_workspace_names(i3, &send).await?;
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
                            update_workspace_names(i3, &send).await?;
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
        while let Some(cmd) = recv.recv().await {
            i3.send_msg_body(Msg::RunCommand, cmd).await?;
        }
        log::debug!("Receiver loop ended");
        Ok::<_, Error>(())
    });

    let (send, recv) = tokio::try_join!(s_handle, r_handle)?;
    send.and(recv)?;
    Ok(())
}
