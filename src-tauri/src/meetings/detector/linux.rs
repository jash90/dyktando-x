//! Linux: PulseAudio/PipeWire recording streams (`pactl list source-outputs`) — program name
//! from `application.process.binary`.
use std::process::Command;

pub fn processes_using_microphone() -> Vec<String> {
    let Ok(out) = Command::new("pactl").args(["list", "source-outputs"]).output() else { return Vec::new() };
    let own = std::process::id().to_string();
    parse(&String::from_utf8_lossy(&out.stdout), &own)
}

/// "Source Output #N" blocks: we take the binary, skipping our own recording (parec with our PID as parent,
/// recognised by the client name "Dyktando X").
pub fn parse(text: &str, _own_pid: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in text.split("Source Output #").skip(1) {
        let field = |key: &str| {
            block.lines().find_map(|l| {
                let l = l.trim();
                l.strip_prefix(key).and_then(|r| r.trim_start().strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string())
            })
        };
        if field("application.name").as_deref() == Some("Dyktando X") {
            continue;
        }
        if let Some(bin) = field("application.process.binary") {
            out.push(bin);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_pactl() {
        let t = "Source Output #12\n\tDriver: protocol-native.c\n\tProperties:\n\t\tapplication.name = \"Firefox\"\n\t\tapplication.process.binary = \"firefox\"\nSource Output #13\n\tProperties:\n\t\tapplication.name = \"Dyktando X\"\n\t\tapplication.process.binary = \"parec\"\n";
        assert_eq!(super::parse(t, "1"), vec!["firefox".to_string()]);
    }
}
