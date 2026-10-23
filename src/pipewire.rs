use std::process::{Command, Output};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sink {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) is_default: bool,
}

pub(crate) const WPCTL: &str = "wpctl";
pub(crate) const STATUS_ARGS: [&str; 2] = ["status", "-n"];

pub(crate) fn query_status() -> std::io::Result<Output> {
    Command::new(WPCTL).args(STATUS_ARGS).output()
}

pub(crate) fn parse_sinks(status: &str) -> Vec<Sink> {
    let mut in_sinks = false;
    let mut sinks = Vec::new();

    for line in status.lines() {
        if line.contains("Sinks:") {
            in_sinks = true;
            continue;
        }
        if line.contains("Sources:") {
            break;
        }
        if !in_sinks {
            continue;
        }

        let is_default = line.contains('*');
        let line = line.replace('*', " ");
        let Some((id, name)) = line.trim().split_once('.') else {
            continue;
        };
        let id = id.trim().trim_matches(|ch: char| !ch.is_ascii_digit());
        let name = name.trim();
        if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) || name.is_empty() {
            continue;
        }

        sinks.push(Sink {
            id: id.to_string(),
            name: name.to_string(),
            is_default,
        });
    }

    sinks
}

pub(crate) fn default_sink(status: &str) -> Option<String> {
    parse_sinks(status)
        .into_iter()
        .find(|sink| sink.is_default)
        .map(|sink| sink.id)
}

#[cfg(test)]
mod tests {
    use super::{default_sink, parse_sinks, Sink};

    const STATUS: &str = "\
Audio\n\
 ├─ Sinks:\n\
 │  * 42. Built-in Audio Analog Stereo [vol: 0.50]\n\
 │    57. USB Audio Device [vol: 0.75]\n\
 ├─ Sources:\n\
 │  * 61. Built-in Audio Analog Stereo\n";

    #[test]
    fn parses_sinks_and_default_without_sources() {
        assert_eq!(
            parse_sinks(STATUS),
            vec![
                Sink {
                    id: "42".into(),
                    name: "Built-in Audio Analog Stereo [vol: 0.50]".into(),
                    is_default: true,
                },
                Sink {
                    id: "57".into(),
                    name: "USB Audio Device [vol: 0.75]".into(),
                    is_default: false,
                },
            ]
        );
        assert_eq!(default_sink(STATUS).as_deref(), Some("42"));
    }

    #[test]
    fn returns_no_default_sink_when_the_status_has_none() {
        assert_eq!(default_sink(" ├─ Sinks:\n │  42. Built-in Audio"), None);
    }
}
