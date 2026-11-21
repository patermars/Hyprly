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
        let name = name
            .split_once(" [vol:")
            .map_or(name, |(name, _)| name)
            .trim();
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

pub(crate) fn selected_sink(status: &str, source: &str) -> Option<Sink> {
    // Older settings store wpctl graph IDs; pw-record accepts node names or serials.
    parse_sinks(status)
        .into_iter()
        .find(|sink| sink.id == source || sink.name == source)
}

#[cfg(test)]
mod tests {
    use super::{parse_sinks, selected_sink, Sink};

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
                    name: "Built-in Audio Analog Stereo".into(),
                    is_default: true,
                },
                Sink {
                    id: "57".into(),
                    name: "USB Audio Device".into(),
                    is_default: false,
                },
            ]
        );
    }

    #[test]
    fn selects_output_monitors_by_node_name_including_saved_device_ids() {
        for source in ["57", "USB Audio Device"] {
            assert_eq!(
                selected_sink(STATUS, source).unwrap().name,
                "USB Audio Device"
            );
        }
        assert!(selected_sink(STATUS, "alsa_input.usb").is_none());
    }
}
