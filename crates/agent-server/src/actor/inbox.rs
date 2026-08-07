//! Dispatch one internal actor inbox message onto the session-owned state.

use tokio::sync::broadcast;

use agent_core::Session;
use agent_runtime::RunnerCtx;

use crate::command::Command;
use crate::event::Frame;

use super::message::ActorMessage;
use super::{commands, remote_tools};

pub(super) enum LoopControl {
    Continue,
    Break,
}

pub(super) fn dispatch(
    message: ActorMessage,
    session: &mut Session,
    ctx: &mut RunnerCtx,
    events: &broadcast::Sender<Frame>,
) -> LoopControl {
    match message {
        ActorMessage::Command(command) => dispatch_command(command, session, ctx, events),
        ActorMessage::ClaimRemoteTool { request, reply } => {
            remote_tools::claim(session, ctx, request, reply);
            LoopControl::Continue
        }
        ActorMessage::SubmitRemoteToolResult { request, reply } => {
            remote_tools::submit(session, ctx, events, request, reply);
            LoopControl::Continue
        }
    }
}

fn dispatch_command(
    command: Command,
    session: &mut Session,
    ctx: &mut RunnerCtx,
    events: &broadcast::Sender<Frame>,
) -> LoopControl {
    match command {
        Command::Input { text } => {
            commands::handle_input(session, ctx, events, &text)
        }
        Command::Undo { granularity, force } => {
            commands::handle_undo(session, ctx, events, granularity, force)
        }
        Command::Redo => commands::handle_redo(session, ctx, events),
        Command::Cancel => commands::handle_cancel(session, ctx, events),
        Command::RemoteToolResult {
            agent,
            call_id,
            content,
            is_error,
        } => commands::handle_remote_tool_result(
            session, ctx, events, agent, call_id, content, is_error,
        ),
        Command::Shutdown => return LoopControl::Break,
    }
    LoopControl::Continue
}
