use anyhow::{Error, Result};
use std;
use tokio::sync::mpsc;
use tokio_i3ipc::{
    event::{Event, Subscribe, WindowChange, WorkspaceChange},
    msg::Msg,
    reply::{Node, NodeLayout, NodeType, Rect},
    I3,
};
use tokio_stream::StreamExt;

// #[rustfmt::skip]
// fn split_rect(r: Rect) -> &'static str {
//     if r.width > r.height { "split h" }
//     else { "split v" }
// }

// walk the tree and determine if `window_id` has tabbed parent
// fn has_tabbed_parent(node: &Node, window_id: usize, tabbed: bool) -> bool {
//     if node.id == window_id {
//         tabbed
//     } else {
//         node.nodes.iter().any(|child| {
//             has_tabbed_parent(
//                 child,
//                 window_id,
//                 matches!(node.layout, NodeLayout::Tabbed | NodeLayout::Stacked),
//             )
//         })
//     }
// }

fn get_workspace_name(node: Node) -> String {
    "Foo".to_string()
}

fn get_nodes_of_type<'a>(node: &'a Node, node_type: NodeType) -> impl Iterator<Item = &'a Node> {
    node.nodes.iter().filter(move |n| n.node_type == node_type)
}

fn get_workspace_nodes(node: &Node) -> impl Iterator<Item = &Node> {
    get_nodes_of_type(node, NodeType::Workspace)
}

fn get_content_nodes(node: &Node) -> impl Iterator<Item = &Node> {
    get_nodes_of_type(node, NodeType::Con)
}

fn get_output_nodes(node: &Node) -> impl Iterator<Item = &Node> {
    get_nodes_of_type(node, NodeType::Output)
}

// fn get_workspace_nodes(node: Node) -> &[Node] {
//     assert!(node.node_type == NodeType::Root);
// }

async fn tree_fun(i3: &mut I3) -> Result<()> {
    let root = i3.get_tree().await?;
    let workspace_nodes = get_output_nodes(&root)
        .map(|n| get_content_nodes(n))
        .flatten()
        .map(|n| get_workspace_nodes(n))
        .flatten();
    //  log::debug!("root: {:#?}", root);
    log::debug!("{:#?}", workspace_nodes.collect::<Vec<&Node>>());
    return Ok(());
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    flexi_logger::Logger::try_with_env()?.start()?;
    let mut i3 = I3::connect().await?;
    log::debug!("Connected!");
    let (send, mut recv) = mpsc::channel::<&'static str>(10);

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
                            // new, close, move, floating (?)
                            log::debug!("Window event");
                            tree_fun(i3).await?;
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
                            log::debug!("Workspace event");
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // while let Some(Ok(Event::Window(window_data))) = event_listener.next().await {

        //     // send.send("move").await?;
        //     //         send.send(split_rect(window_data.container.window_rect))
        //     //             .await?;
        //     // }
        // }
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
