use std::io::{self, Write};

pub fn read_masked(prompt: &str) -> Option<String> {
    print!("{}", prompt);
    let _ = io::stdout().flush();
    read_terminal().or_else(read_line)
}

fn read_line() -> Option<String> {
    let mut password = String::new();
    io::stdin().read_line(&mut password).ok()?;
    Some(password.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(windows)]
fn read_terminal() -> Option<String> {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(number: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn ReadConsoleW(
            handle: *mut c_void,
            buffer: *mut u16,
            length: u32,
            read: *mut u32,
            reserved: *mut c_void,
        ) -> i32;
    }

    struct Mode {
        handle: *mut c_void,
        original: u32,
    }

    impl Drop for Mode {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.handle, self.original);
            }
        }
    }

    let handle = unsafe { GetStdHandle((-10i32) as u32) };
    if handle.is_null() || handle as isize == -1 {
        return None;
    }
    let mut original = 0;
    if unsafe { GetConsoleMode(handle, &mut original) } == 0 {
        return None;
    }
    if unsafe { SetConsoleMode(handle, original & !0x0001 & !0x0002 & !0x0004) } == 0 {
        return None;
    }
    let _mode = Mode { handle, original };
    let mut password = String::new();
    loop {
        let mut value = 0u16;
        let mut count = 0u32;
        let ok = unsafe {
            ReadConsoleW(
                handle,
                &mut value,
                1,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || count != 1 {
            return None;
        }
        match value {
            3 => {
                println!();
                return Some(String::new());
            }
            8 => {
                if password.pop().is_some() {
                    print!("\u{8} \u{8}");
                    let _ = io::stdout().flush();
                }
            }
            13 => {
                println!();
                return Some(password);
            }
            value => {
                if let Some(character) = char::from_u32(value as u32) {
                    password.push(character);
                    print!("*");
                    let _ = io::stdout().flush();
                }
            }
        }
    }
}

#[cfg(unix)]
fn read_terminal() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    struct Mode(String);

    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = Command::new("stty")
                .arg(&self.0)
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    let output = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let original = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let status = Command::new("stty")
        .args(["-echo", "-icanon", "-isig", "min", "1", "time", "0"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let _mode = Mode(original);
    let mut password = Vec::new();
    let mut input = io::stdin();
    loop {
        let mut value = [0u8; 1];
        input.read_exact(&mut value).ok()?;
        match value[0] {
            3 => {
                println!();
                return Some(String::new());
            }
            8 | 127 => {
                if password.pop().is_some() {
                    print!("\u{8} \u{8}");
                    let _ = io::stdout().flush();
                }
            }
            10 | 13 => {
                println!();
                return String::from_utf8(password).ok();
            }
            value => {
                password.push(value);
                print!("*");
                let _ = io::stdout().flush();
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
fn read_terminal() -> Option<String> {
    None
}
