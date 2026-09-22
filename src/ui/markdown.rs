fn escape_html(input: &str) -> String {
    input
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
}

pub fn markdown_to_pango(input: &str) -> String {
    let mut output = String::new();
    let mut in_code_block = false;

    for line in input.lines() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            let escaped = escape_html(line);
            output.push_str(&format!("<tt>{}</tt>\n", escaped));
            continue;
        }

        let mut processed = line.to_string();

        if processed.starts_with("### ") {
            processed = format!("<span weight=\"bold\">{}</span>", escape_html(&processed[4..]));
        } else if processed.starts_with("## ") {
            processed = format!("<span size=\"large\" weight=\"bold\">{}</span>", escape_html(&processed[3..]));
        } else if processed.starts_with("# ") {
            processed = format!("<span size=\"x-large\" weight=\"bold\">{}</span>", escape_html(&processed[2..]));
        } else if processed.starts_with("- ") {
            processed = format!("  • {}", escape_html(&processed[2..]));
        } else {
            processed = escape_html(&processed);
            
            while let Some(start) = processed.find("**") {
                if let Some(end) = processed[start + 2..].find("**") {
                    let end_idx = start + 2 + end;
                    let inner = &processed[start + 2..end_idx];
                    processed = format!("{}<b>{}</b>{}", &processed[..start], inner, &processed[end_idx + 2..]);
                } else {
                    break;
                }
            }

            while let Some(start) = processed.find("*") {
                if let Some(end) = processed[start + 1..].find("*") {
                    let end_idx = start + 1 + end;
                    let inner = &processed[start + 1..end_idx];
                    processed = format!("{}<i>{}</i>{}", &processed[..start], inner, &processed[end_idx + 1..]);
                } else {
                    break;
                }
            }

            while let Some(start) = processed.find("`") {
                if let Some(end) = processed[start + 1..].find("`") {
                    let end_idx = start + 1 + end;
                    let inner = &processed[start + 1..end_idx];
                    processed = format!("{}<tt>{}</tt>{}", &processed[..start], inner, &processed[end_idx + 1..]);
                } else {
                    break;
                }
            }
        }

        output.push_str(&processed);
        output.push('\n');
    }

    if output.ends_with('\n') {
        output.pop();
    }

    output
}
