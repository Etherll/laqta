//! Run at login, by one of two entries. The default is the `Laqta` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. When Laqta runs as administrator and is
//! installed where only an administrator can change it (Program Files), it is a Task Scheduler logon
//! task with the highest privileges instead, which starts it with those rights and no UAC prompt.
//! Anywhere else (a per-user install, a portable copy) any program the user runs could replace the
//! file the task starts and so gain administrator rights at the next sign-in; there the Run value
//! stays, and Laqta asks for the rights through the UAC prompt as it starts.
//! Turning `run_as_admin` on or off moves the entry from one to the other.
//! The task comes from an XML definition rather than `schtasks` switches: the switches leave Task
//! Scheduler's defaults, which stop the program after 72 hours and do not start it on battery power.
//! The value text, the task XML and the `schtasks` arguments are pure and tested on every target;
//! the registry and process calls are Windows-only.

use std::path::Path;

#[cfg(windows)]
use laqta_ports::{Autostart, PortError};

#[cfg(windows)]
use crate::integration::OpenKey;

#[cfg(windows)]
const VALUE_NAME: &str = "Laqta";

/// The name of the logon task.
const TASK_NAME: &str = "Laqta";

/// The Run value's data for `executable`: the quoted path and `-silent`, so the start stays quiet.
pub(crate) fn run_command(executable: &Path) -> String {
    format!("\"{}\" -silent", executable.display())
}

/// `text` for the inside of an XML element.
fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// The logon task as Task Scheduler XML: it starts `executable -silent` when `user` signs in, with the
/// highest privileges that user has (no UAC prompt at sign-in), never stops it on a time limit, runs
/// it on battery power, and does not start a second copy. `user` is a SID or `DOMAIN\name`.
///
/// Only SYSTEM, the Administrators group and the user (when `user` is a SID) may change or remove
/// the task, so no other account can point it at another program.
pub(crate) fn task_xml(executable: &Path, user: &str) -> String {
    let security = xml_escape(&task_sddl(user));
    let user = xml_escape(user);
    let command = xml_escape(&executable.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <SecurityDescriptor>{security}</SecurityDescriptor>
    <Description>Starts Laqta when you sign in, with administrator rights.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>-silent</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Who may read and change the task: full access for SYSTEM, the Administrators group and the user
/// it is for, when `user` is a SID (the non-elevated copy removes the task when sign-in start is
/// turned off). Nobody else gets any.
fn task_sddl(user: &str) -> String {
    let mut sddl = "D:(A;;FA;;;SY)(A;;FA;;;BA)".to_owned();
    if user.starts_with("S-1-") && user[2..].chars().all(|c| c.is_ascii_digit() || c == '-') {
        sddl.push_str(&format!("(A;;FA;;;{user})"));
    }
    sddl
}

/// Whether `executable` lies inside one of `protected` (folders only an administrator can change,
/// such as Program Files). Letter case does not matter, and a path that climbs out with `..` is never inside.
pub(crate) fn in_protected_folder(executable: &Path, protected: &[std::path::PathBuf]) -> bool {
    // Split by hand: Windows separators are not separators to `Path` on other targets.
    if executable.to_string_lossy().split(['\\', '/']).any(|part| part == "..") {
        return false;
    }
    let lower = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let exe = lower(executable);
    protected.iter().map(|dir| lower(dir)).any(|dir| !dir.is_empty() && exe.starts_with(&format!("{dir}\\")))
}

/// Program Files, as Windows names it (not the environment, which the user's programs can change).
#[cfg(windows)]
fn protected_folders() -> Vec<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86]
        .iter()
        .filter_map(|id| {
            // SAFETY: the returned string is copied and then freed with CoTaskMemFree, as the API asks.
            unsafe {
                let path = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
                let text = path.to_string().ok();
                CoTaskMemFree(Some(path.0.cast()));
                text.map(std::path::PathBuf::from)
            }
        })
        .collect()
}

/// `xml` as the file `schtasks /XML` reads: UTF-16 little-endian with a byte order mark, as its
/// declaration says.
pub(crate) fn task_file_bytes(xml: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// Who the task is for: the SID when it is known, otherwise `DOMAIN\name` from the environment.
pub(crate) fn task_user(sid: Option<String>, domain: Option<String>, name: Option<String>) -> Option<String> {
    let non_empty = |text: Option<String>| text.filter(|text| !text.is_empty());
    non_empty(sid).or_else(|| {
        let name = non_empty(name)?;
        Some(match non_empty(domain) {
            Some(domain) => format!("{domain}\\{name}"),
            None => name,
        })
    })
}

/// `schtasks` arguments that create the logon task from the XML in `xml_file`.
pub(crate) fn create_task_args(xml_file: &Path) -> Vec<String> {
    ["/Create", "/F", "/TN", TASK_NAME, "/XML"].into_iter().map(str::to_owned).chain([xml_file.display().to_string()]).collect()
}

/// `schtasks` arguments that delete the logon task.
pub(crate) fn delete_task_args() -> Vec<String> {
    ["/Delete", "/F", "/TN", TASK_NAME].into_iter().map(str::to_owned).collect()
}

/// `schtasks` arguments that look the logon task up: success means it exists.
pub(crate) fn query_task_args() -> Vec<String> {
    ["/Query", "/TN", TASK_NAME].into_iter().map(str::to_owned).collect()
}

/// `schtasks`, from the system folder: this runs elevated, so it must not come from the search path.
#[cfg(windows)]
fn schtasks(args: &[String]) -> Result<std::process::Output, PortError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let program = std::env::var_os("SystemRoot")
        .map(|root| std::path::Path::new(&root).join("System32").join("schtasks.exe"))
        .unwrap_or_else(|| "schtasks.exe".into());
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| PortError::Failed(format!("schtasks: {e}")))
}

/// Runs `schtasks` and turns a non-zero exit into an error that carries its message.
#[cfg(windows)]
fn schtasks_checked(args: &[String]) -> Result<(), PortError> {
    let output = schtasks(args)?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr });
    Err(PortError::Failed(format!("schtasks: {}", message.trim())))
}

/// The task definition in a temporary file for `schtasks /XML`, deleted on drop. This runs elevated and
/// the temporary folder is the user's, so another process of the same user must not be able to swap
/// the file for one that starts something else with administrator rights. It is made new (never an
/// existing file), read back, and then held open with other writers and deleters shut out until
/// `schtasks` has read it.
#[cfg(windows)]
struct TaskFile {
    path: std::path::PathBuf,
    held: Option<std::fs::File>,
}

#[cfg(windows)]
impl TaskFile {
    fn write(bytes: &[u8]) -> std::io::Result<TaskFile> {
        use std::io::{Read, Write};
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let path = std::env::temp_dir().join(format!("laqta-task-{}-{stamp}.xml", std::process::id()));
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?.write_all(bytes)?;
        let mut file = TaskFile { path, held: None };
        let mut held = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&file.path)?;
        let mut read_back = Vec::with_capacity(bytes.len());
        held.read_to_end(&mut read_back)?;
        if read_back != bytes {
            return Err(std::io::Error::other("the task file changed while it was being written"));
        }
        file.held = Some(held);
        Ok(file)
    }
}

#[cfg(windows)]
impl Drop for TaskFile {
    fn drop(&mut self) {
        drop(self.held.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Creates (or replaces) the logon task for the current user.
#[cfg(windows)]
fn create_task(executable: &Path) -> Result<(), PortError> {
    let env = |name: &str| std::env::var(name).ok();
    let user = task_user(crate::elevation::current_user_sid(), env("USERDOMAIN"), env("USERNAME"))
        .ok_or_else(|| PortError::Failed("cannot tell which user the sign-in task is for".to_owned()))?;
    let bytes = task_file_bytes(&task_xml(executable, &user));
    let file = TaskFile::write(&bytes).map_err(|e| PortError::Failed(format!("cannot write the sign-in task definition: {e}")))?;
    schtasks_checked(&create_task_args(&file.path))
}

#[cfg(windows)]
fn task_exists() -> bool {
    schtasks(&query_task_args()).is_ok_and(|output| output.status.success())
}

#[cfg(windows)]
fn remove_task() -> Result<(), PortError> {
    if task_exists() { schtasks_checked(&delete_task_args()) } else { Ok(()) }
}

/// The Run key under `HKCU`, and the value Laqta keeps there.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsAutostart {
    run_key: String,
}

#[cfg(windows)]
impl Default for WindowsAutostart {
    fn default() -> Self {
        Self { run_key: r"Software\Microsoft\Windows\CurrentVersion\Run".to_owned() }
    }
}

#[cfg(windows)]
impl WindowsAutostart {
    fn run_value_present(&self) -> Result<bool, PortError> {
        match OpenKey::open(&self.run_key)? {
            Some(key) => key.has_value(VALUE_NAME),
            None => Ok(false),
        }
    }

    fn write_run_value(&self, executable: &Path) -> Result<(), PortError> {
        OpenKey::create(&self.run_key)?.set_string(Some(VALUE_NAME), &run_command(executable))
    }

    fn remove_run_value(&self) -> Result<(), PortError> {
        match OpenKey::open(&self.run_key)? {
            Some(key) => key.delete_value(VALUE_NAME),
            None => Ok(()),
        }
    }

    /// Keeps exactly one entry: the logon task when `elevated`, otherwise the Run value. The new entry
    /// is made before the old one is removed, so a failure (creating the task needs an elevated
    /// process) leaves the start at sign-in as it was.
    fn set(&self, enabled: bool, elevated: bool, executable: &Path) -> Result<(), PortError> {
        match (enabled, elevated) {
            // A task that starts a file other programs can replace would hand them administrator
            // rights at sign-in, so outside Program Files the Run value stays (and the copy it starts
            // asks for the rights through the UAC prompt).
            (true, true) if !in_protected_folder(executable, &protected_folders()) => {
                tracing::info!(exe = %executable.display(), "not in Program Files: starting at sign-in through the Run key and the UAC prompt");
                self.write_run_value(executable)?;
                remove_task()
            }
            // Already the logon task alone: nothing to rewrite. The installed copy keeps one path
            // (`current\laqta.exe`), so a task that exists points at it.
            (true, true) if !self.run_value_present()? && task_exists() => Ok(()),
            (true, true) => {
                create_task(executable)?;
                self.remove_run_value()
            }
            (true, false) => {
                self.write_run_value(executable)?;
                remove_task()
            }
            (false, _) => {
                let run = self.remove_run_value();
                let task = remove_task();
                run.and(task)
            }
        }
    }
}

#[cfg(windows)]
impl Autostart for WindowsAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        Ok(self.run_value_present()? || task_exists())
    }

    /// Keeps the kind of entry that is there: a logon task stays a logon task.
    fn set_enabled(&self, enabled: bool, executable: &Path) -> Result<(), PortError> {
        self.set(enabled, enabled && task_exists(), executable)
    }

    fn set_enabled_elevated(&self, enabled: bool, elevated: bool, executable: &Path) -> Result<(), PortError> {
        self.set(enabled, elevated, executable)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        create_task_args, delete_task_args, in_protected_folder, query_task_args, run_command, task_file_bytes, task_user, task_xml,
        xml_escape,
    };

    #[test]
    fn only_a_copy_inside_program_files_counts_as_protected() {
        let protected = [std::path::PathBuf::from(r"C:\Program Files"), std::path::PathBuf::from(r"C:\Program Files (x86)\")];
        assert!(in_protected_folder(Path::new(r"C:\Program Files\Laqta\laqta.exe"), &protected));
        assert!(in_protected_folder(Path::new(r"c:\program files (x86)\Laqta\laqta.exe"), &protected));
        assert!(!in_protected_folder(Path::new(r"C:\Users\me\AppData\Local\Laqta\current\laqta.exe"), &protected));
        assert!(!in_protected_folder(Path::new(r"C:\Program Files Evil\laqta.exe"), &protected), "a sibling with the same start");
        assert!(!in_protected_folder(Path::new(r"C:\Program Files\..\Users\me\laqta.exe"), &protected));
        assert!(!in_protected_folder(Path::new(r"C:\Program Files\Laqta\laqta.exe"), &[]));
    }

    #[test]
    fn only_system_administrators_and_the_user_may_change_the_task() {
        let xml = task_xml(Path::new(r"C:\Program Files\Laqta\laqta.exe"), "S-1-5-21-1-2-3-1001");
        assert!(xml.contains("<SecurityDescriptor>D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;S-1-5-21-1-2-3-1001)</SecurityDescriptor>"), "{xml}");
        let xml = task_xml(Path::new(r"C:\Program Files\Laqta\laqta.exe"), r"HOME\me");
        assert!(xml.contains("<SecurityDescriptor>D:(A;;FA;;;SY)(A;;FA;;;BA)</SecurityDescriptor>"), "{xml}");
    }

    #[test]
    fn run_value_quotes_the_path_and_starts_silently() {
        assert_eq!(run_command(Path::new(r"C:\Program Files\Laqta\laqta.exe")), r#""C:\Program Files\Laqta\laqta.exe" -silent"#);
    }

    #[test]
    fn the_logon_task_has_the_privileges_and_settings_the_defaults_would_spoil() {
        let xml = task_xml(Path::new(r"C:\Program Files\Laqta\laqta.exe"), "S-1-5-21-1-2-3-1001");
        for expected in [
            "<LogonTrigger>",
            "<UserId>S-1-5-21-1-2-3-1001</UserId>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>HighestAvailable</RunLevel>",
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            r"<Command>C:\Program Files\Laqta\laqta.exe</Command>",
            "<Arguments>-silent</Arguments>",
        ] {
            assert!(xml.contains(expected), "missing {expected}\n{xml}");
        }
        assert_eq!(xml.matches("<UserId>S-1-5-21-1-2-3-1001</UserId>").count(), 2, "the trigger and the principal");
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#));
    }

    #[test]
    fn the_path_and_user_are_escaped_in_the_task_xml() {
        let xml = task_xml(Path::new(r#"C:\Tom & Jerry\<beta> "x"\it's.exe"#), r"HOME\a&b");
        assert!(xml.contains(r"<Command>C:\Tom &amp; Jerry\&lt;beta&gt; &quot;x&quot;\it&apos;s.exe</Command>"), "{xml}");
        assert!(xml.contains(r"<UserId>HOME\a&amp;b</UserId>"), "{xml}");
        assert!(!xml.contains("Tom & Jerry"));
    }

    #[test]
    fn xml_text_escapes_every_markup_character() {
        assert_eq!(xml_escape(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&apos;");
        assert_eq!(xml_escape("plain C:\\path"), "plain C:\\path");
    }

    #[test]
    fn the_task_file_is_utf16_little_endian_with_a_byte_order_mark() {
        assert_eq!(task_file_bytes("<a>é</a>"), [0xFF, 0xFE, b'<', 0, b'a', 0, b'>', 0, 0xE9, 0, b'<', 0, b'/', 0, b'a', 0, b'>', 0]);
        assert_eq!(task_file_bytes(""), [0xFF, 0xFE]);
    }

    #[test]
    fn the_task_is_for_the_sid_or_else_domain_and_name() {
        let some = |text: &str| Some(text.to_owned());
        assert_eq!(task_user(some("S-1-5-21-1-2-3-1001"), some("HOME"), some("ann")), some("S-1-5-21-1-2-3-1001"));
        assert_eq!(task_user(None, some("HOME"), some("ann")), some(r"HOME\ann"));
        assert_eq!(task_user(some(""), None, some("ann")), some("ann"));
        assert_eq!(task_user(None, some("HOME"), None), None);
        assert_eq!(task_user(None, None, some("")), None);
    }

    #[test]
    fn the_task_is_created_from_a_file_and_deleted_and_queried_by_name() {
        assert_eq!(create_task_args(Path::new(r"C:\Temp\task.xml")), ["/Create", "/F", "/TN", "Laqta", "/XML", r"C:\Temp\task.xml"]);
        assert_eq!(delete_task_args(), ["/Delete", "/F", "/TN", "Laqta"]);
        assert_eq!(query_task_args(), ["/Query", "/TN", "Laqta"]);
    }
}
