//! Running as administrator: whether this process is elevated, and a relaunch through the UAC
//! prompt. The command-line quoting and the SID text are pure and tested on every target; the
//! token and shell calls are Windows-only.

#[cfg(windows)]
use std::io;

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
#[cfg(windows)]
use windows::Win32::Security::{GetLengthSid, GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser};
#[cfg(windows)]
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
#[cfg(windows)]
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
#[cfg(windows)]
use windows::core::{HSTRING, PCWSTR, w};

/// `arg` as one argument of a Windows command line, the inverse of what `CommandLineToArgvW` reads:
/// quoted when it is empty or holds a space, tab or quote, with the backslashes before a quote doubled.
#[cfg(any(windows, test))]
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes at the end sit before the closing quote.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// The arguments joined into one command-line tail.
#[cfg(any(windows, test))]
fn command_line(args: &[String]) -> String {
    args.iter().map(|arg| quote_arg(arg)).collect::<Vec<_>>().join(" ")
}

/// A binary SID as text (`S-1-5-21-…`): the revision, the 48-bit authority, then each sub-authority.
/// `None` when the bytes are shorter than the SID they describe.
#[cfg(any(windows, test))]
fn sid_string(sid: &[u8]) -> Option<String> {
    let (&revision, rest) = sid.split_first()?;
    let (&count, rest) = rest.split_first()?;
    let authority = rest.get(..6)?.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
    let subs = rest.get(6..6 + 4 * usize::from(count))?;
    let mut text = format!("S-{revision}-{authority}");
    for sub in subs.as_chunks::<4>().0 {
        text.push('-');
        text.push_str(&u32::from_le_bytes(*sub).to_string());
    }
    Some(text)
}

/// The current process token, closed on drop.
#[cfg(windows)]
struct Token(HANDLE);

#[cfg(windows)]
impl Token {
    fn open() -> io::Result<Token> {
        let mut handle = HANDLE::default();
        // SAFETY: `GetCurrentProcess` is a pseudo handle that needs no closing, and `handle` outlives the call.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) }.map_err(io::Error::from)?;
        Ok(Token(handle))
    }
}

#[cfg(windows)]
impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle came from `OpenProcessToken` and is closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Whether this process runs with an elevated (administrator) token. False when that cannot be read.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    Token::open().is_ok_and(|token| token_is_elevated(&token))
}

/// Whether process `pid` runs elevated. False when that cannot be read (the process is gone, or
/// belongs to another user).
#[cfg(windows)]
pub fn process_is_elevated(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain handle calls; each handle opened here is closed by `Token`'s drop or below.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut handle = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut handle);
        let _ = CloseHandle(process);
        if opened.is_err() {
            return false;
        }
        token_is_elevated(&Token(handle))
    }
}

#[cfg(windows)]
fn token_is_elevated(token: &Token) -> bool {
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the buffer is a `TOKEN_ELEVATION` and the length passed is its size.
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    read.is_ok() && elevation.TokenIsElevated != 0
}

/// The SID of the user this process runs as, as text. The IPC pipe grants it access.
#[cfg(windows)]
pub fn current_user_sid() -> Option<String> {
    let token = Token::open().ok()?;
    let mut needed = 0u32;
    // SAFETY: a size query with no buffer; it fails with the size in `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    // u64 elements keep the buffer aligned for `TOKEN_USER`.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer holds at least `needed` bytes.
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buffer.as_mut_ptr().cast()), needed, &mut needed) }.ok()?;
    // SAFETY: the call filled the buffer with a `TOKEN_USER`, whose SID points into the same buffer, so
    // both stay valid while `buffer` lives.
    let bytes = unsafe {
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let length = GetLengthSid(user.User.Sid) as usize;
        std::slice::from_raw_parts(user.User.Sid.0.cast::<u8>(), length)
    };
    sid_string(bytes)
}

/// Starts this program again with administrator rights, through the UAC prompt, with `args`.
/// `Ok(true)` means the elevated copy started and is still running a few seconds later, so this one
/// should exit; `Ok(false)` means the user declined the prompt. An elevated copy that ends at once
/// (it could not start its window, say) is an error, so this copy goes on without the rights.
#[cfg(windows)]
pub fn relaunch_elevated(args: &[String]) -> io::Result<bool> {
    relaunch(args, true)
}

/// `relaunch_elevated` for a running copy that hands over to the elevated one: `Ok(true)` as soon as
/// the user accepts the prompt, without watching the new copy, which waits for this one to quit.
#[cfg(windows)]
pub fn relaunch_elevated_now(args: &[String]) -> io::Result<bool> {
    relaunch(args, false)
}

#[cfg(windows)]
fn relaunch(args: &[String], watch: bool) -> io::Result<bool> {
    let exe = HSTRING::from(std::env::current_exe()?.as_os_str());
    let parameters = HSTRING::from(command_line(args));
    // The elevated copy resolves relative paths in its arguments against the same folder.
    let directory = std::env::current_dir().ok().map(|dir| HSTRING::from(dir.as_os_str()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: directory.as_ref().map_or(PCWSTR::null(), |dir| PCWSTR(dir.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to outlive the call.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) if watch => wait_for_start(info.hProcess),
        Ok(()) => {
            if !info.hProcess.is_invalid() {
                // SAFETY: the handle ShellExecuteExW returned for this call, closed once.
                let _ = unsafe { CloseHandle(info.hProcess) };
            }
            Ok(true)
        }
        Err(error) if error.code() == ERROR_CANCELLED.to_hresult() => Ok(false),
        Err(error) => Err(io::Error::from(error)),
    }
}

/// Watches the elevated copy for its first seconds: still running is a start, an exit is a failure.
#[cfg(windows)]
fn wait_for_start(process: windows::Win32::Foundation::HANDLE) -> io::Result<bool> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    if process.is_invalid() {
        return Ok(true);
    }
    // SAFETY: `process` is the handle ShellExecuteExW returned for this call; it is closed once.
    unsafe {
        let ended = WaitForSingleObject(process, 6000) == WAIT_OBJECT_0;
        let mut code = 0u32;
        if ended {
            let _ = GetExitCodeProcess(process, &raw mut code);
        }
        let _ = CloseHandle(process);
        if ended { Err(io::Error::other(format!("the copy with administrator rights ended at once (exit code {code})"))) } else { Ok(true) }
    }
}

#[cfg(test)]
mod tests {
    use super::{command_line, quote_arg, sid_string};

    #[test]
    fn plain_arguments_stay_bare() {
        assert_eq!(quote_arg("--action"), "--action");
        assert_eq!(quote_arg("-silent"), "-silent");
    }

    #[test]
    fn spaces_empty_and_quotes_are_quoted() {
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r"C:\My Pictures\a.png"), r#""C:\My Pictures\a.png""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn backslashes_before_a_quote_or_the_end_are_doubled() {
        assert_eq!(quote_arg(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        assert_eq!(quote_arg(r#"a\"b c"#), r#""a\\\"b c""#);
        assert_eq!(quote_arg(r"C:\NoSpaces\"), r"C:\NoSpaces\");
    }

    #[test]
    fn a_command_line_joins_the_quoted_arguments() {
        let args = ["--action".to_owned(), "open-settings".to_owned(), r"C:\My Files\x.png".to_owned()];
        assert_eq!(command_line(&args), r#"--action open-settings "C:\My Files\x.png""#);
        assert_eq!(command_line(&[]), "");
    }

    #[test]
    fn a_sid_reads_as_the_usual_text() {
        // Local System: revision 1, one sub-authority, authority 5 (NT), sub-authority 18.
        assert_eq!(sid_string(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]).as_deref(), Some("S-1-5-18"));
        // A domain user: S-1-5-21-1-2-3-1001.
        let mut user = vec![1, 5, 0, 0, 0, 0, 0, 5];
        for sub in [21u32, 1, 2, 3, 1001] {
            user.extend(sub.to_le_bytes());
        }
        assert_eq!(sid_string(&user).as_deref(), Some("S-1-5-21-1-2-3-1001"));
    }

    #[test]
    fn a_truncated_sid_is_refused() {
        assert_eq!(sid_string(&[]), None);
        assert_eq!(sid_string(&[1, 2, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0]), None);
    }
}
