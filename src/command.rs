#[derive(Debug, Clone)]
pub enum ExCommand {
    Write,
    WriteAs(String),
    Quit,
    QuitForce,
    WriteQuit,
    Edit(String),
    Goto(usize),
    Unknown(String),
}

pub fn parse(line: &str) -> ExCommand {
    let line = line.trim();
    if line.is_empty() {
        return ExCommand::Unknown(String::new());
    }
    // Bare line number: ":42"
    if let Ok(n) = line.parse::<usize>() {
        return ExCommand::Goto(n);
    }
    let (head, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    match head {
        "w" | "write" => {
            if rest.is_empty() {
                ExCommand::Write
            } else {
                ExCommand::WriteAs(rest.to_string())
            }
        }
        "q" | "quit" => ExCommand::Quit,
        "q!" | "quit!" => ExCommand::QuitForce,
        "wq" | "x" => ExCommand::WriteQuit,
        "e" | "edit" => ExCommand::Edit(rest.to_string()),
        _ => ExCommand::Unknown(line.to_string()),
    }
}
