//! What loom and everything it started use in memory (PSS: shared pages split among the
//! processes sharing them, so the parts add up to the whole). Read from /proc; for the debug
//! log, every few minutes.

use std::collections::HashMap;

/// An agent as the snapshot needs it: (id, "project · title", state, its adapter's pid).
pub type AgentInfo = (u32, String, String, Option<u32>);

/// One line per part: the total, loom's window, each agent with its processes, the rest.
/// `previous`: the last total (MB), to point out a jump. Returns the text and the total.
pub fn snapshot(agents: &[AgentInfo], previous: Option<u64>) -> (String, u64) {
    let me = std::process::id();
    let (parents, names) = processes();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, parent) in &parents {
        children.entry(*parent).or_default().push(*pid);
    }
    let below = |root: u32| -> Vec<u32> {
        let mut found = vec![root];
        let mut i = 0;
        while i < found.len() {
            if let Some(kids) = children.get(&found[i]) {
                found.extend(kids);
            }
            i += 1;
        }
        found
    };
    let ours: Vec<u32> = below(me).into_iter().filter(|p| *p != me).collect();
    let window = pss(me);
    let mut lines = Vec::new();
    let mut counted: Vec<u32> = Vec::new();
    let mut agents_total = 0;
    let mut states: HashMap<&str, usize> = HashMap::new();
    for (id, name, state, pid) in agents {
        *states.entry(state.as_str()).or_default() += 1;
        // Suspended: nothing running (counted in the first line only).
        let Some(pid) = pid else { continue };
        let tree = below(*pid);
        let mut parts: Vec<(String, u64)> = tree.iter().map(|p| (names.get(p).cloned().unwrap_or_default(), pss(*p))).collect();
        parts.sort_by(|a, b| b.1.cmp(&a.1));
        let total: u64 = parts.iter().map(|(_, kb)| kb).sum();
        agents_total += total;
        counted.extend(&tree);
        let shown: Vec<String> = parts.iter().filter(|(_, kb)| *kb >= 1024).map(|(n, kb)| format!("{n} {}", mb(*kb))).collect();
        lines.push(format!("  agent {id} {name} ({state}): {} in {} processes [{}]", mb(total), tree.len(), shown.join(", ")));
    }
    let others: Vec<u32> = ours.iter().copied().filter(|p| !counted.contains(p)).collect();
    let mut other_parts: Vec<(String, u64)> = others.iter().map(|p| (names.get(p).cloned().unwrap_or_default(), pss(*p))).collect();
    other_parts.sort_by(|a, b| b.1.cmp(&a.1));
    let other_total: u64 = other_parts.iter().map(|(_, kb)| kb).sum();
    let total = window + agents_total + other_total;
    let mut state_words: Vec<String> = states.iter().map(|(s, n)| format!("{n} {s}")).collect();
    state_words.sort();
    let mut head = format!(
        "memory: {} in all · window {} · {} agents ({}) {} · other {} ({} processes{})",
        mb(total),
        mb(window),
        agents.len(),
        state_words.join(", "),
        mb(agents_total),
        mb(other_total),
        others.len(),
        if other_parts.is_empty() { String::new() } else { format!(": {}", other_parts.iter().take(4).map(|(n, kb)| format!("{n} {}", mb(*kb))).collect::<Vec<_>>().join(", ")) },
    );
    let total_mb = total / 1024;
    if let Some(before) = previous.filter(|b| *b > 0 && total_mb > b + b * 3 / 10) {
        head = format!("SPIKE from {before} MB · {head}");
    }
    lines.insert(0, head);
    (lines.join("\n"), total_mb)
}

fn mb(kb: u64) -> String {
    format!("{} MB", (kb + 512) / 1024)
}

/// Every process's parent and name.
fn processes() -> (HashMap<u32, u32>, HashMap<u32, String>) {
    let (mut parents, mut names) = (HashMap::new(), HashMap::new());
    let Ok(entries) = std::fs::read_dir("/proc") else { return (parents, names) };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // "pid (name) state ppid …": the name may hold spaces and parentheses.
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[open + 1..close].to_owned();
        let Some(parent) = stat[close + 1..].split_whitespace().nth(1).and_then(|p| p.parse::<u32>().ok()) else { continue };
        parents.insert(pid, parent);
        names.insert(pid, name);
    }
    (parents, names)
}

/// A process's proportional memory, in kB (0 if gone or unreadable).
fn pss(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .ok()
        .and_then(|text| text.lines().find(|l| l.starts_with("Pss:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse().ok()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_is_measured() {
        let (text, total) = super::snapshot(&[], None);
        assert!(text.starts_with("memory: "));
        assert!(total > 0);
    }
}
