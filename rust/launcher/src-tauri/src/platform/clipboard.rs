//! `copyText`: the page has no clipboard permission, so an invite link is copied here (RD22, RA11). The
//! command checks the 4096-character limit (`bridge::spec::COPY_TEXT_MAX`).
//!
//! Windows: the clipboard API, as Unicode text. Linux: the desktop's own tool, given the text on stdin
//! (`wl-copy` on Wayland, else `xclip` or `xsel`), so no clipboard crate or plugin comes in.

/// Puts `text` on the system clipboard.
pub fn write_text(text: &str) -> std::io::Result<()> {
    os::write_text(text)
}

#[cfg(windows)]
mod os {
    #![allow(
        unsafe_code,
        reason = "the clipboard is a C API; each call is wrapped and its memory handed over or freed here"
    )]

    use std::time::Duration;

    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    /// `CF_UNICODETEXT`
    const UNICODE_TEXT: u32 = 13;

    fn copy_into(memory: HGLOBAL, units: &[u16]) -> std::io::Result<()> {
        // SAFETY: `memory` was allocated with room for `units` and is locked only here
        unsafe {
            let target = GlobalLock(memory).cast::<u16>();
            if target.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
            // Fails with no error once the lock count is back to zero, which is the expected outcome
            let _ = GlobalUnlock(memory);
        }
        Ok(())
    }

    fn set(units: &[u16]) -> std::io::Result<()> {
        // SAFETY: the clipboard is open (`write_text`); the memory goes to the clipboard on success and is
        // freed here otherwise
        unsafe {
            EmptyClipboard()?;
            let memory = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(units))?;
            let placed = copy_into(memory, units)
                .and_then(|()| SetClipboardData(UNICODE_TEXT, Some(HANDLE(memory.0))).map_err(Into::into));
            if placed.is_err() {
                let _ = GlobalFree(Some(memory));
            }
            placed.map(|_| ())
        }
    }

    pub fn write_text(text: &str) -> std::io::Result<()> {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // Another program can hold the clipboard for a moment
        let mut opened = Err(std::io::Error::other("clipboard busy"));
        for _ in 0..10 {
            // SAFETY: no owner window; closed below
            opened = unsafe { OpenClipboard(None) }.map_err(Into::into);
            if opened.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        opened?;
        let result = set(&units);
        // SAFETY: opened above
        let _ = unsafe { CloseClipboard() };
        result
    }
}

#[cfg(not(windows))]
mod os {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    /// The tools to try, in order: Wayland's own first when the session is Wayland.
    pub(super) fn tools(wayland: bool) -> Vec<(&'static str, &'static [&'static str])> {
        let mut tools: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        if wayland {
            tools.push(("wl-copy", &[]));
        }
        tools.push(("xclip", &["-selection", "clipboard"]));
        tools.push(("xsel", &["--clipboard", "--input"]));
        tools
    }

    fn copy_with(program: &str, args: &[&str], text: &str) -> std::io::Result<()> {
        // stdout and stderr closed: xclip and wl-copy leave a child serving the clipboard, which must not
        // hold a pipe of ours open
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(text.as_bytes()));
        let status = child.wait()?;
        written?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{program} failed")))
        }
    }

    pub fn write_text(text: &str) -> std::io::Result<()> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty());
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no clipboard tool");
        for (program, args) in tools(wayland) {
            match copy_with(program, args, text) {
                Ok(()) => return Ok(()),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::os::tools;

    #[test]
    fn wayland_sessions_try_wl_copy_first() {
        let names = |wayland| tools(wayland).into_iter().map(|(name, _)| name).collect::<Vec<_>>();
        assert_eq!(names(true), ["wl-copy", "xclip", "xsel"]);
        assert_eq!(names(false), ["xclip", "xsel"]);
    }
}
