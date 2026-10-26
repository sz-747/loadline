//! Process-tree accounting. Pure: works on a process snapshot taken by `sys`.

use std::collections::{HashMap, HashSet};

/// Executable names (lowercase, no `.exe`) treated as agent roots by `loadline top`.
pub const AGENT_NAMES: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "cursor-agent",
    "amp",
    "goose",
    "qwen",
    "crush",
    "droid",
    "pi",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Lowercase executable name without `.exe`.
    pub name: String,
    /// Committed private bytes (`PrivateUsage`); 0 when the process could not be opened.
    pub private: u64,
    pub working_set: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TreeUsage {
    pub root: u32,
    pub name: String,
    pub processes: usize,
    pub private: u64,
    pub working_set: u64,
}

pub struct Snapshot<'a> {
    procs: &'a [Proc],
    by_pid: HashMap<u32, usize>,
    children: HashMap<u32, Vec<usize>>,
}

impl<'a> Snapshot<'a> {
    pub fn new(procs: &'a [Proc]) -> Self {
        let mut by_pid = HashMap::new();
        let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, p) in procs.iter().enumerate() {
            by_pid.insert(p.pid, i);
            // pid 0 parents itself; never link a process to itself.
            if p.ppid != p.pid {
                children.entry(p.ppid).or_default().push(i);
            }
        }
        Self {
            procs,
            by_pid,
            children,
        }
    }

    /// The pids of `root` and all its descendants. The visited set guards against
    /// cycles created by pid reuse (a stale ppid pointing at a newer process).
    pub fn descendants(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        if !self.by_pid.contains_key(&root) {
            return out;
        }
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            out.push(pid);
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids.iter().map(|&i| self.procs[i].pid));
            }
        }
        out
    }

    pub fn usage(&self, root: u32) -> Option<TreeUsage> {
        let root_proc = &self.procs[*self.by_pid.get(&root)?];
        let pids = self.descendants(root);
        let (mut private, mut working_set) = (0, 0);
        for pid in &pids {
            let p = &self.procs[self.by_pid[pid]];
            private += p.private;
            working_set += p.working_set;
        }
        Some(TreeUsage {
            root,
            name: root_proc.name.clone(),
            processes: pids.len(),
            private,
            working_set,
        })
    }

    /// Agent-named processes with no agent-named ancestor and outside `exclude`
    /// (trees already shown, such as guarded runs).
    pub fn agent_roots(&self, names: &[&str], exclude: &HashSet<u32>) -> Vec<u32> {
        let is_agent = |i: usize| names.contains(&self.procs[i].name.as_str());
        let mut roots = Vec::new();
        for (i, p) in self.procs.iter().enumerate() {
            if !is_agent(i) || exclude.contains(&p.pid) {
                continue;
            }
            let mut seen = HashSet::from([p.pid]);
            let mut parent = p.ppid;
            let mut nested = false;
            while let Some(&pi) = self.by_pid.get(&parent) {
                if !seen.insert(parent) {
                    break;
                }
                if is_agent(pi) || exclude.contains(&parent) {
                    nested = true;
                    break;
                }
                parent = self.procs[pi].ppid;
            }
            if !nested {
                roots.push(p.pid);
            }
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str, private: u64) -> Proc {
        Proc {
            pid,
            ppid,
            name: name.into(),
            private,
            working_set: private / 2,
        }
    }

    fn sample() -> Vec<Proc> {
        vec![
            p(1, 0, "explorer", 100),
            p(10, 1, "windowsterminal", 50),
            p(20, 10, "claude", 1000),
            p(21, 20, "node", 300),
            p(22, 21, "rg", 10),
            p(30, 10, "codex", 400),
            p(31, 30, "codex", 40), // nested codex helper, not a separate root
            p(40, 10, "pwsh", 80),
        ]
    }

    #[test]
    fn sums_whole_tree() {
        let procs = sample();
        let snap = Snapshot::new(&procs);
        let u = snap.usage(20).unwrap();
        assert_eq!(u.processes, 3);
        assert_eq!(u.private, 1310);
        assert_eq!(u.working_set, 500 + 150 + 5);
    }

    #[test]
    fn finds_top_level_agents_only() {
        let procs = sample();
        let snap = Snapshot::new(&procs);
        let mut roots = snap.agent_roots(AGENT_NAMES, &HashSet::new());
        roots.sort();
        assert_eq!(roots, vec![20, 30]);
    }

    #[test]
    fn excludes_guarded_trees() {
        let procs = sample();
        let snap = Snapshot::new(&procs);
        let roots = snap.agent_roots(AGENT_NAMES, &HashSet::from([10]));
        assert!(roots.is_empty());
    }

    #[test]
    fn survives_pid_reuse_cycle() {
        let procs = vec![p(5, 6, "claude", 10), p(6, 5, "node", 20)];
        let snap = Snapshot::new(&procs);
        assert_eq!(snap.usage(5).unwrap().processes, 2);
        assert_eq!(snap.agent_roots(AGENT_NAMES, &HashSet::new()), vec![5]);
    }

    #[test]
    fn missing_root_is_none() {
        let procs = sample();
        assert!(Snapshot::new(&procs).usage(999).is_none());
    }
}
