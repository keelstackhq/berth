use crate::error::BerthError;
use crate::manifest::Entry;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Ok,
    Remove,
    Drift,
    Refused,
    Skipped,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Ok => "ok",
            Action::Remove => "remove",
            Action::Drift => "drift",
            Action::Refused => "refused",
            Action::Skipped => "skipped",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Action::Create | Action::Ok => "32",
            Action::Remove | Action::Skipped => "36",
            Action::Drift => "33",
            Action::Refused => "31",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub kind: String,
    pub key: String,
    pub action: Action,
    pub reason: String,
}

impl Item {
    pub fn new(
        kind: impl Into<String>,
        key: impl Into<String>,
        action: Action,
        reason: impl Into<String>,
    ) -> Self {
        Item {
            kind: kind.into(),
            key: key.into(),
            action,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub command: String,
    pub items: Vec<Item>,
    pub worktrees: Vec<Entry>,
    pub exit_code: i32,
    /// Printed instead of the table (`env`); never serialized.
    pub text: Option<String>,
    /// Printed on its own line after the table (`up`); never serialized.
    pub hint: Option<String>,
}

impl Report {
    pub fn new(command: impl Into<String>) -> Self {
        Report {
            command: command.into(),
            items: Vec::new(),
            worktrees: Vec::new(),
            exit_code: 0,
            text: None,
            hint: None,
        }
    }

    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }

    /// The `--json` report for a failed command: one `error` item, exit code of the error.
    pub fn from_error(command: impl Into<String>, err: &BerthError) -> Self {
        let mut report = Report::new(command);
        report.exit_code = err.exit_code();
        report
            .items
            .push(Item::new("error", err.key(), Action::Refused, err.reason()));
        report
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    version: u32,
    command: &'a str,
    ok: bool,
    exit_code: i32,
    items: &'a [Item],
    worktrees: &'a [Entry],
}

/// The report as one pretty JSON object (no trailing newline).
pub fn render_json(report: &Report) -> String {
    let view = JsonReport {
        version: 1,
        command: &report.command,
        ok: report.ok(),
        exit_code: report.exit_code,
        items: &report.items,
        worktrees: &report.worktrees,
    };
    serde_json::to_string_pretty(&view).expect("report serializes")
}

/// Human table (kind, key, action, reason), then the hint line if any. Ends with a newline.
pub fn render_table(report: &Report, color: bool) -> String {
    let mut out = String::new();
    if report.items.is_empty() {
        out.push_str("no items\n");
    } else {
        let mut rows: Vec<[String; 4]> = vec![[
            "KIND".into(),
            "KEY".into(),
            "ACTION".into(),
            "REASON".into(),
        ]];
        for item in &report.items {
            rows.push([
                item.kind.clone(),
                item.key.clone(),
                item.action.as_str().to_string(),
                item.reason.clone(),
            ]);
        }
        let mut widths = [0usize; 3];
        for row in &rows {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell.chars().count());
            }
        }
        for (i, row) in rows.iter().enumerate() {
            let action = format!("{:<w$}", row[2], w = widths[2]);
            let action = match (color, i) {
                (true, n) if n > 0 => {
                    format!(
                        "\x1b[{}m{action}\x1b[0m",
                        report.items[n - 1].action.color()
                    )
                }
                _ => action,
            };
            out.push_str(&format!(
                "{:<w0$}  {:<w1$}  {action}  {}\n",
                row[0],
                row[1],
                row[3],
                w0 = widths[0],
                w1 = widths[1],
            ));
        }
    }
    if let Some(hint) = &report.hint {
        out.push_str(hint);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_report_json() {
        let report = Report::from_error("up", &BerthError::Env("x".into()));
        let value: serde_json::Value = serde_json::from_str(&render_json(&report)).unwrap();
        assert_eq!(value["exit_code"], 3);
        assert_eq!(value["ok"], false);
        assert_eq!(value["command"], "up");
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "error");
        assert_eq!(items[0]["key"], "env");
        assert_eq!(items[0]["action"], "refused");
        assert_eq!(items[0]["reason"], "x");
    }

    #[test]
    fn hint_follows_table() {
        let mut report = Report::new("up");
        report
            .items
            .push(Item::new("worktree", "fix-login", Action::Create, "ready"));
        report.hint = Some("cd ../repo.berth/fix-login".into());
        report.text = Some("TEXT".into());
        let table = render_table(&report, false);
        assert!(table.ends_with("cd ../repo.berth/fix-login\n"), "{table}");
        assert!(table.contains("fix-login") && table.contains("create"));
        let json = render_json(&report);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("hint").is_none() && value.get("text").is_none());
        assert!(!json.contains("TEXT"));
    }

    #[test]
    fn json_field_order() {
        let json = render_json(&Report::new("ls"));
        let pos = |k: &str| json.find(&format!("\"{k}\"")).unwrap();
        let order = [
            "version",
            "command",
            "ok",
            "exit_code",
            "items",
            "worktrees",
        ];
        assert!(order.windows(2).all(|w| pos(w[0]) < pos(w[1])));
    }
}
