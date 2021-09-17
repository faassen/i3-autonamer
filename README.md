# i3-autonamer

(The project is still under construction)

This is my attempt at writing an automatic workspace renamer for the I3 window
manager. It listens to i3ipc events and renames workspaces based on what's in
them. So, if you have Firefox in a workspace, the workspace is renamed
according to rules you specify.

I've used Rust, Tokio, and tokio-i3ipc. The chosen tools are total overkill for
the job, but it's been a fun exercise.
