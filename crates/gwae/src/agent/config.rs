//! Config text editing: comment-preserving TOML key writes.

use super::detect::{command_available, Found};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `default_agent` resolved; exec it with no UI at all.
    Configured(String),
    /// `default_agent` is set but missing; offer these instead (never empty).
    Missing { want: String, found: Vec<Found> },
    /// Nothing configured, but harnesses exist; let the user pick.
    Choose(Vec<Found>),
    /// Nothing configured and nothing installed; fall back to a shell.
    NoneInstalled { want: Option<String> },
}

/// Decide what `gwae agent` should do for this `default_agent` setting.
pub fn plan(default_agent: &str, found: Vec<Found>) -> Plan {
    let want = default_agent.trim();
    if !want.is_empty() && command_available(want) {
        return Plan::Configured(want.to_string());
    }
    if found.is_empty() {
        return Plan::NoneInstalled {
            want: (!want.is_empty()).then(|| want.to_string()),
        };
    }
    if want.is_empty() {
        Plan::Choose(found)
    } else {
        Plan::Missing {
            want: want.to_string(),
            found,
        }
    }
}

/// The shell to fall back to when there is no harness to run.
pub fn fallback_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}

/// Rewrite `default_agent` in a config file *without* disturbing anything
/// else: an existing top-level assignment is replaced in place, otherwise the
/// line is appended. Comments, key order, and formatting all survive, which a
/// parse/re-serialize round trip would destroy.
///
/// Only a top-level key is touched. Lines inside a `[table]` are skipped, so a
/// `default_agent` under some future section can never be clobbered.
pub fn set_default_agent_text(text: &str, agent: &str) -> String {
    set_scalar_text(text, "default_agent", &toml_string(agent))
}

/// As [`set_default_agent_text`], for any top-level key. `value` must already
/// be valid TOML (quoted for strings, bare for numbers), so the same
/// comment-preserving rewrite serves both the agent gateway and the latency
/// tuner rather than each growing its own config writer.
pub fn set_scalar_text(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut out: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut replaced = false;
    for raw in text.lines() {
        let t = raw.trim_start();
        if t.starts_with('[') {
            in_table = true;
        }
        let is_key = !in_table
            && !replaced
            && t.strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);
        if is_key {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        // Append before the first table header if there is one, since a bare
        // key after `[theme]` would silently become `theme.default_agent`.
        let at = out
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .unwrap_or(out.len());
        let at = if at > 0 && !out[at.saturating_sub(1)].trim().is_empty() {
            out.insert(at, String::new());
            out.insert(at + 1, line);
            at + 1
        } else {
            out.insert(at, line);
            at
        };
        // Keep a blank line between the key and a following table header, so
        // repeated writes cannot glue `key = v` onto `[table]` and make the
        // file read as if the key were inside it.
        if out.get(at + 1).map(|l| l.trim_start().starts_with('[')) == Some(true) {
            out.insert(at + 1, String::new());
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// [`toml_string`] for callers outside this module (the `⌥+d` picker writes
/// `agent_dir` through the same comment-preserving path).
pub fn toml_string_pub(s: &str) -> String {
    toml_string(s)
}

/// Comment-preserving rewrite for `harness_dirs.<key> = "dir"`.
///
/// TOML allows `harness_dirs = { jcode = "..." }` and the dotted form
/// `[harness_dirs]` + `jcode = "..."` and inline mutations. The cheap
/// approach that handles each without a full parser is to do three passes:
/// 1) replace a dotted `harness_dirs.<key> = ...` line anywhere,
/// 2) replace inside an existing inline table `harness_dirs = { … }`,
/// 3) otherwise append/insert a dotted assignment after any existing
///    `[harness_dirs]` block or at the file's top-level tail.
///
/// Preserves comments/order; the file stays valid TOML.
pub fn set_harness_dir_text(text: &str, key: &str, dir: &str) -> String {
    let dotted = format!("harness_dirs.{key}");
    // 1. dotted form anywhere.
    let as_dotted = set_scalar_text(text, &dotted, &toml_string(dir));
    if as_dotted != text && as_dotted.contains(&format!("{dotted} =")) {
        // We actually replaced a dotted line; done.
        // Detect via whether the new text differs and contains the key now.
        // If user had no dotted line, set_scalar_text just appended one —
        // we still want to prefer inline table editing when one exists, so
        // only early-return when the original had a dotted line.
        if text.lines().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#')
                && !t.starts_with('[')
                && t.starts_with(&dotted)
                && t[dotted.len()..].trim_start().starts_with('=')
        }) {
            return as_dotted;
        }
    }
    // 2. inline table `harness_dirs = { ... }`
    let lines: Vec<&str> = text.lines().collect();
    for (idx, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        if t.starts_with('[') {
            continue;
        }
        // Look for `harness_dirs = {` on this line.
        let Some(eq) = t.find('=') else { continue };
        let lhs = t[..eq].trim();
        if lhs != "harness_dirs" {
            continue;
        }
        let rhs = t[eq + 1..].trim_start();
        if !(rhs.starts_with('{') && rhs.contains('}')) {
            continue;
        }
        // Replace or insert `key = "dir"` inside the braces, preserving prior content.
        // Parse naively: extract inside `{ }`.
        let lbrace = t.find('{').unwrap();
        let rbrace = t.rfind('}').unwrap();
        let inside = &t[lbrace + 1..rbrace];
        let val = toml_string(dir);
        // If key already there, replace its value; else append.
        let mut replaced = false;
        let mut parts: Vec<String> = Vec::new();
        // Split on commas not inside quotes (values are strings, so crude split on ',' is fine).
        let mut cur = String::new();
        let mut in_q = false;
        let mut esc = false;
        for ch in inside.chars() {
            if esc {
                cur.push(ch);
                esc = false;
                continue;
            }
            if ch == '\\' && in_q {
                cur.push(ch);
                esc = true;
                continue;
            }
            if ch == '"' {
                in_q = !in_q;
                cur.push(ch);
                continue;
            }
            if ch == ',' && !in_q {
                parts.push(cur);
                cur = String::new();
                continue;
            }
            cur.push(ch);
        }
        if !cur.trim().is_empty() || inside.contains(',') {
            parts.push(cur);
        }
        for p in parts.iter_mut() {
            let trimmed = p.trim_start();
            // Extract key before `=`
            if let Some(eq2) = trimmed.find('=') {
                let k = trimmed[..eq2].trim().trim_matches('"').trim();
                // Unquote dotted key form key.
                if k == key {
                    *p = format!(" {key} = {val} ");
                    replaced = true;
                }
            }
        }
        let new_inside = if replaced {
            parts.join(",")
        } else {
            if inside.trim().is_empty() {
                format!(" {key} = {val} ")
            } else {
                let trimmed = inside.trim_end();
                let sep = if trimmed.ends_with(',') { " " } else { ", " };
                format!("{inside}{sep}{key} = {val} ")
            }
        };
        let prefix = &raw[..raw.find('{').unwrap() + 1];
        let suffix = &raw[raw.rfind('}').unwrap()..];
        // Rebuild line preserving leading indent and trailing comment outside braces? Keep simple: just the braces content.
        // Preserve any leading indent from original.
        let indent_len = raw.len() - raw.trim_start().len();
        let indent = &raw[..indent_len];
        let new_line = format!("{indent}harness_dirs = {{{new_inside}}}");
        let _ = (prefix, suffix); // not used but keep signature clear
        let mut out: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        out[idx] = new_line;
        let mut s = out.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // 3. No inline table; if a `[harness_dirs]` section exists, add/replace `key =` inside it.
    if text.lines().any(|l| l.trim() == "[harness_dirs]") {
        let mut out: Vec<String> = Vec::new();
        let mut in_harness = false;
        let mut replaced = false;
        for raw in text.lines() {
            let t = raw.trim();
            if t.starts_with('[') {
                in_harness = t == "[harness_dirs]";
                out.push(raw.to_string());
                continue;
            }
            if in_harness && !replaced {
                let trimmed = raw.trim_start();
                if !trimmed.starts_with('#') && !trimmed.is_empty() {
                    if let Some(eq) = trimmed.find('=') {
                        let k = trimmed[..eq].trim().trim_matches('"').trim();
                        if k == key {
                            let indent_len = raw.len() - raw.trim_start().len();
                            let indent = &raw[..indent_len];
                            out.push(format!("{indent}{key} = {}", toml_string(dir)));
                            replaced = true;
                            continue;
                        }
                    }
                }
            }
            out.push(raw.to_string());
        }
        if !replaced {
            // Append inside the section, before next table or EOF.
            let mut inserted = false;
            let mut out2: Vec<String> = Vec::new();
            let mut in_harness2 = false;
            for (i, raw) in out.iter().enumerate() {
                if raw.trim() == "[harness_dirs]" {
                    in_harness2 = true;
                } else if raw.trim_start().starts_with('[') {
                    if in_harness2 && !inserted {
                        out2.push(format!("{key} = {}", toml_string(dir)));
                        inserted = true;
                    }
                    in_harness2 = false;
                }
                out2.push(raw.clone());
                // If at EOF and still in harness section.
                if i == out.len() - 1 && in_harness2 && !inserted {
                    out2.push(format!("{key} = {}", toml_string(dir)));
                    inserted = true;
                }
            }
            let mut s = out2.join("\n");
            if !s.ends_with('\n') {
                s.push('\n');
            }
            return s;
        }
        let mut s = out.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // 4. No existing harness_dirs at all: insert a dotted assignment like default_agent does,
    //    before the first table header if any.
    let line = format!("harness_dirs.{key} = {}", toml_string(dir));
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    // Reuse dotted insertion path: the `as_dotted` above already handled appending a dotted line;
    // but we short-circuited only when original had a dotted line, so this path adds it now.
    // Prefer inline table for first write: `harness_dirs = { jcode = "..." }`
    let inline = format!("harness_dirs = {{ {key} = {} }}", toml_string(dir));
    for raw in text.lines() {
        let t = raw.trim_start();
        if t.starts_with('[') && !replaced {
            // Insert before first table.
            if out
                .last()
                .map(|l: &String| !l.trim().is_empty())
                .unwrap_or(false)
            {
                out.push(String::new());
            }
            out.push(inline.clone());
            out.push(String::new());
            replaced = true;
        }
        out.push(raw.to_string());
    }
    if !replaced {
        if !out.is_empty() && !out.last().unwrap().trim().is_empty() {
            out.push(String::new());
        }
        out.push(inline);
    }
    let _ = line;
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Quote a value as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Persist `default_agent` to `path`, creating the file (and its parent) when
/// it does not exist yet.
pub fn save_default_agent(path: &Path, agent: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = if existing.trim().is_empty() {
        format!(
            "# gwae configuration\n{}\n",
            format_args!("default_agent = {}", toml_string(agent))
        )
    } else {
        set_default_agent_text(&existing, agent)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::*;

    /// Drop SGR escapes so assertions read the text a user sees.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn found(cmd: &str) -> Found {
        Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: PathBuf::from("/usr/bin").join(cmd),
        }
    }

        #[test]
    fn saving_replaces_an_existing_key_and_preserves_comments_and_order() {
        let before = "# my config\nstartup_panes = 1\ndefault_agent = \"jcode\"\nmouse = true\n";
        let after = set_default_agent_text(before, "claude");
        assert_eq!(
            after,
            "# my config\nstartup_panes = 1\ndefault_agent = \"claude\"\nmouse = true\n"
        );
    }

    #[test]
    fn saving_appends_when_absent_and_stays_above_any_table_header() {
        let before = "startup_panes = 1\n\n[theme]\npreset = \"nord\"\n";
        let after = set_default_agent_text(before, "claude");
        // Must land before `[theme]`, or it would become theme.default_agent.
        let agent_at = after.find("default_agent").unwrap();
        let table_at = after.find("[theme]").unwrap();
        assert!(agent_at < table_at, "{after}");
        assert!(after.contains("preset = \"nord\""));
        // And it must still parse as the value we asked for.
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn saving_into_a_flat_file_appends_at_the_end() {
        let after = set_default_agent_text("startup_panes = 1\n", "codex");
        assert_eq!(after, "startup_panes = 1\n\ndefault_agent = \"codex\"\n");
    }

    #[test]
    fn a_default_agent_inside_a_table_is_never_clobbered() {
        let before = "[theme]\ndefault_agent = \"decoy\"\n";
        let after = set_default_agent_text(before, "claude");
        assert!(after.contains("default_agent = \"decoy\""));
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["default_agent"].as_str(), Some("decoy"));
    }

    #[test]
    fn saved_values_are_quoted_so_odd_commands_round_trip() {
        let after = set_default_agent_text("", "my agent");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("my agent"));
    }

    #[test]
    fn save_creates_the_file_and_its_parent_directory() {
        let dir = std::env::temp_dir().join(format!("gwae-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/gwae.toml");
        save_default_agent(&path, "jcode").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("jcode"));
        // Saving again over the fresh file replaces rather than duplicates.
        save_default_agent(&path, "claude").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("default_agent").count(), 1);
        assert!(text.contains("claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

}
