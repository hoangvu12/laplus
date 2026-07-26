# 17 — Terminal: open, write, resize

**What to build:** A developer opens a terminal rooted in their project directory
and uses it as a normal shell alongside the agent. Interactive programs render
correctly, and resizing the pane rewraps output the way a real terminal does.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** ready-for-agent

- [ ] A terminal opens with the project directory as its working directory
- [ ] Typed input reaches the shell and output streams back
- [ ] Interactive full-screen programs render correctly
- [ ] Colour output renders correctly
- [ ] Resizing the pane resizes the underlying terminal and output rewraps
- [ ] High-volume output does not stall or disconnect the socket
- [ ] The shell's exit is reported in the UI
- [ ] Tests drive open, write and resize through the socket boundary and assert
      the streamed output
