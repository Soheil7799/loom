//! A commit graph's lanes, as the web page's `lib/lanes.ts` lays them out (the window draws
//! the same): commits newest first, each after all its children (`git log --date-order`),
//! one row each, in a lane (a column). A line from a commit to a parent runs down a lane: its
//! first parent's goes on in the commit's own lane (or joins a lane to its left already
//! waiting for that parent), another parent's joins the lane waiting for it or takes a free
//! one. Lanes waiting for one parent meet where it is (the leftmost), as git draws them. A
//! parent not loaded yet: the line runs to the bottom (`to` None).

/// From the commit in lane `from` at `row`, down lane `lane`, to the parent at row `to` in lane `at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub row: usize,
    pub lane: usize,
    pub to: Option<usize>,
    pub at: usize,
}

#[derive(Debug, Default)]
pub struct Layout {
    /// Each commit's lane, by row.
    pub lanes: Vec<usize>,
    pub edges: Vec<Edge>,
    /// How many lanes it takes.
    pub width: usize,
}

/// The lanes of `commits`: (hash, parents), newest first.
pub fn layout<S: AsRef<str>>(commits: &[(S, Vec<S>)]) -> Layout {
    // each lane: the hash it waits for, and the edges (indexes) running down it to that commit
    let mut open: Vec<Option<(String, Vec<usize>)>> = Vec::new();
    let mut out = Layout::default();
    let free = |open: &Vec<Option<(String, Vec<usize>)>>, not: Option<usize>| open.iter().enumerate().position(|(k, l)| l.is_none() && Some(k) != not).unwrap_or(open.len());
    for (row, (hash, parents)) in commits.iter().enumerate() {
        let hash = hash.as_ref();
        let waiting: Vec<usize> = open.iter().enumerate().filter(|(_, l)| l.as_ref().is_some_and(|(h, _)| h == hash)).map(|(k, _)| k).collect();
        let lane = waiting.first().copied().unwrap_or_else(|| free(&open, None));
        for k in waiting {
            if let Some((_, edges)) = open[k].take() {
                for e in edges {
                    out.edges[e].to = Some(row);
                    out.edges[e].at = lane;
                }
            }
        }
        out.lanes.push(lane);
        for (n, p) in parents.iter().enumerate() {
            let p = p.as_ref();
            let there = open.iter().position(|l| l.as_ref().is_some_and(|(h, _)| h == p));
            let into = match there {
                Some(k) if n > 0 || k < lane => k,
                _ if n == 0 => lane,
                _ => free(&open, Some(lane)),
            };
            out.edges.push(Edge { from: lane, row, lane: into, to: None, at: into });
            let e = out.edges.len() - 1;
            if open.len() <= into {
                open.resize(into + 1, None);
            }
            match &mut open[into] {
                Some((h, edges)) if h == p => edges.push(e),
                slot => *slot = Some((p.to_owned(), vec![e])),
            }
        }
    }
    out.width = out.lanes.iter().chain(out.edges.iter().map(|e| &e.lane)).map(|l| l + 1).max().unwrap_or(1).max(1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &'static str, parents: &[&'static str]) -> (&'static str, Vec<&'static str>) {
        (hash, parents.to_vec())
    }
    fn shape(l: &Layout) -> Vec<(usize, usize, usize, Option<usize>, usize)> {
        l.edges.iter().map(|e| (e.from, e.row, e.lane, e.to, e.at)).collect()
    }

    #[test]
    fn lanes_as_the_web_lays_them_out() {
        let straight = layout(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!((straight.lanes.clone(), straight.width), (vec![0, 0, 0], 1));
        // m merges f into main (b); f and b both come from a: they meet there, as git draws it
        let merged = layout(&[c("m", &["b", "f"]), c("f", &["a"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(merged.lanes, vec![0, 1, 0, 0]);
        assert_eq!(shape(&merged), vec![(0, 0, 0, Some(2), 0), (0, 0, 1, Some(1), 1), (1, 1, 1, Some(3), 0), (0, 2, 0, Some(3), 0)]);
        assert_eq!(merged.width, 2);
        // two tips, two lanes; a parent not loaded runs to the bottom
        let tips = layout(&[c("x", &["o"]), c("y", &["p"])]);
        assert_eq!(tips.lanes, vec![0, 1]);
        assert!(tips.edges.iter().all(|e| e.to.is_none()));
    }
}
