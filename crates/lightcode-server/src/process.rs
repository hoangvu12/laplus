//! Starting child processes the way this server needs them started.
//!
//! Small, and it exists because the same four lines were about to be written a
//! third time. The server shells out to `git` for the file tree
//! ([`crate::filesystem`]), starts the developer's editor
//! ([`crate::editor`]), and will drive the `claude` CLI and a shell in the
//! tickets after this one. Every one of them has the same Windows problem.

use std::process::Command;

/// Start this child without giving it a console window.
///
/// On Windows a process started from a GUI application gets a console of its
/// own unless it is told not to, so without this a black window flashes on
/// screen every time the file tree is scanned or an editor is launched — once
/// the server is inside the Tauri shell (ticket 23), which is where it will
/// spend its life. It is a visible bug for something the user never asked to
/// see.
///
/// A no-op everywhere else: the flag is a Windows creation flag and no other
/// platform has the problem.
pub fn without_a_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`, from the Windows process-creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
