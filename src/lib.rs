//! vtee: a terminal multiplexer for AI and for testing.
//!
//! Like `tmux`, it runs a program in a pseudo-terminal and keeps the terminal's state. Unlike `tmux`, the
//! point is not a human switching between panes: it is that a program or an AI agent can ask what the
//! terminal looks like right now, and (later) type into it, while the human, if there is one, keeps using
//! their own terminal normally. Think of it as `tee` for a TUI.
//!
//! Nothing is implemented yet. See the README for the requirements and the staged plan, and HANDOFF.md for
//! the research and context.
