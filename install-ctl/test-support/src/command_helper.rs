use std::{
    env,
    io::{self, Read},
    process::{Command, ExitCode},
};

fn append(path: &str, record: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fixture log");
    writeln!(file, "{record}").expect("write fixture log");
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(result, "\\u{:04x}", character as u32).expect("write JSON escape");
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn main() -> ExitCode {
    let mut args = env::args_os();
    args.next();
    match args.next().as_deref() {
        Some(mode) if mode == "credential" => {
            let log = args.next().expect("credential log").to_string_lossy().into_owned();
            let action = args.next().expect("credential action").to_string_lossy().into_owned();
            let mut input = String::new();
            io::stdin().read_to_string(&mut input).expect("read credential input");
            let _ = input;
            append(&log, &format!(r#"{{"action": {action:?}}}"#));
            if action == "get" {
                print!("username=fixture-user\npassword=fixture-password\n\n");
            }
            ExitCode::SUCCESS
        }
        Some(mode) if mode == "ssh" => {
            let log = args.next().expect("SSH log").to_string_lossy().into_owned();
            let arguments: Vec<_> = args.map(|argument| argument.to_string_lossy().into_owned()).collect();
            let evidence = arguments.iter().map(|argument| json_string(argument)).collect::<Vec<_>>().join(",");
            append(&log, &format!(r#"{{"arguments":[{evidence}]}}"#));
            let command = if arguments.get(arguments.len().saturating_sub(2))
                .is_some_and(|argument| argument == "git-upload-pack")
            {
                format!(
                    "git-upload-pack {}",
                    arguments.last().expect("SSH path")
                )
            } else {
                arguments.last().expect("SSH command").to_owned()
            };
            let path = command
                .strip_prefix("git-upload-pack ")
                .expect("fixture only permits git-upload-pack");
            let path = path.trim_matches('\'').trim_matches('"');
            let path = if path.len() > 3 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' {
                &path[1..]
            } else {
                path
            };
            ExitCode::from(
                Command::new("git")
                    .args(["upload-pack", path])
                    .status()
                    .expect("run real git upload-pack")
                    .code()
                    .unwrap_or(1) as u8,
            )
        }
        _ => {
            eprintln!("unknown install-ctl fixture helper mode");
            ExitCode::from(2)
        }
    }
}
