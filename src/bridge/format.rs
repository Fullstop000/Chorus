use serde_json::Value;

pub(super) fn to_local_time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

pub(super) fn format_target(m: &Value) -> String {
    let channel_type = m.get("channel_type").and_then(|v| v.as_str()).unwrap_or("");
    let channel_name = m.get("channel_name").and_then(|v| v.as_str()).unwrap_or("");

    if channel_type == "dm" {
        return format!("dm:@{}", channel_name);
    }
    format!("#{}", channel_name)
}

pub(super) fn format_attachments(attachments: Option<&Value>) -> String {
    match attachments.and_then(|a| a.as_array()) {
        Some(arr) if !arr.is_empty() => {
            let count = arr.len();
            let details: Vec<String> = arr
                .iter()
                .map(|a| {
                    let filename = a.get("filename").and_then(|v| v.as_str()).unwrap_or("file");
                    let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("{} (id:{})", filename, id)
                })
                .collect();
            let plural = if count > 1 { "s" } else { "" };
            format!(
                " [{} image{}: {} \u{2014} use view_file to see]",
                count,
                plural,
                details.join(", ")
            )
        }
        _ => String::new(),
    }
}
