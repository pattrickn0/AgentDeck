//! Memory ruler: measures what each way of delivering learnings would have got right in the merged tasks.
//!
//! ```text
//! cargo run -p holt-core --example memeval -- <repository> <tasks file> [options]
//! ```
//!
//! The `<tasks file>` is the `data/tasks/<project>.json` of Holt. Read-only: the program calls the installed
//! `git` and prints tables; nothing is written.
//!
//! For each case (a merged task, or a direct commit of `dev` with a big enough diff):
//!
//! - **The right one:** the learnings (with `touches`) that match the files the case actually changed, read **as
//!   they were in the base** of the case, so the answer does not leak from the future. The global ones stay out of
//!   the account (they always arrive).
//! - **The guess:** what each strategy would deliver knowing only the request (the `brief`, or the commit message):
//!   (a) today: the learnings of the paths cited in the request; (b) today + the one-line index of every other one;
//!   (c) co-change: the cited paths, widened by the files that changed together with them before the base.
//! - **The score:** recall, precision and delivered size (characters and tokens, characters ÷ 4).
//!
//! It also measures the file prediction (the cited ones + the co-change neighbours against the ones the case
//! changed) and the seed (how many requests cite some path of the repository).
//!
//! Options: `--commits-min-arquivos N` (default 3: minimum size, without `memory/**` and `Cargo.lock`, of the
//! commits of the second sample), `--detalhe` (one line per case).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;

use holt_core::learnings::{self, Learning};

/// Commits with more files than this (reformattings, renames) do not teach co-change.
const MAX_COMMIT_FILES: usize = 15;
/// Budget of characters of learnings in full, the same as the real delivery at the start of a task.
const BUDGET: usize = learnings::START_BUDGET;
/// The same limits with which `learnings::load` reads a learning.
const MAX_LEARNING_BYTES: u64 = 64 * 1024;
/// Half-lives (in days) tested for the decay by age; `None` = no decay.
const HALF_LIVES: [Option<f64>; 3] = [None, Some(7.0), Some(3.0)];
const KS: [usize; 3] = [3, 5, 10];
const MIN_COUNTS: [u32; 3] = [1, 2, 3];
const MIN_SCORES: [f64; 2] = [0.0, 0.3];

/// One row of the grid: where the neighbours come from (half-life and ordering by lift, or `None` = the control),
/// how many, the minimum of times together and the minimum weight.
type Setting = (Option<(usize, bool)>, usize, u32, f64);

// ---------- git ----------

struct Repo {
    dir: PathBuf,
}

impl Repo {
    fn bytes(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(["-c", "core.quotepath=false"])
            .args(args)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(out.stdout)
    }

    fn text(&self, args: &[&str]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.bytes(args)?).into_owned())
    }
}

// ---------- History and co-change ----------

/// Files that enter neither the co-change nor the prediction: the memory and what the cargo rewrites on its own.
fn is_noise(path: &str) -> bool {
    path.starts_with("memory/") || path == "Cargo.lock"
}

struct Commit {
    time: i64,
    files: Vec<String>,
}

/// The commits (without merges) that serve the co-change, by hash, and the file → commits index.
#[derive(Default)]
struct History {
    commits: HashMap<String, Commit>,
    by_file: HashMap<String, Vec<String>>,
}

/// Reads the output of `git log --no-merges --name-only --format=%x01%H %ct`. The commit with more than
/// [`MAX_COMMIT_FILES`] files stays out, as do the noise files; the commit that had only noise disappears.
fn parse_history(log: &str) -> History {
    let mut hist = History::default();
    for chunk in log.split('\u{1}').skip(1) {
        let mut lines = chunk.lines();
        let mut head = lines.next().unwrap_or("").split_whitespace();
        let (Some(hash), Some(Ok(time))) = (head.next(), head.next().map(str::parse::<i64>)) else { continue };
        let raw: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();
        if raw.len() > MAX_COMMIT_FILES {
            continue;
        }
        let files: Vec<String> = raw.into_iter().filter(|f| !is_noise(f)).map(str::to_owned).collect();
        if files.is_empty() {
            continue;
        }
        for f in &files {
            hist.by_file.entry(f.clone()).or_default().push(hash.to_owned());
        }
        hist.commits.insert(hash.to_owned(), Commit { time, files });
    }
    hist
}

/// Weight of a commit with `age_days` of age: 1 without decay, or halved every `half_life` days.
fn weight(age_days: f64, half_life: Option<f64>) -> f64 {
    match half_life {
        None => 1.0,
        Some(h) => 0.5f64.powf(age_days.max(0.0) / h),
    }
}

/// A file that usually changes together with the seeds.
#[derive(Debug, Clone, PartialEq)]
struct Neighbour {
    path: String,
    /// Sum, over the seeds, of (weight of the commits in which they changed together ÷ weight of the commits of the seed).
    score: f64,
    /// How many times they changed together, in the seed where that repeats the most.
    count: u32,
    /// The confidence divided by the frequency of the file in all commits: the more specific to the seed, the higher.
    lift: f64,
}

/// The files that changed together with the `seeds` in the commits that are ancestors of the base (`ancestors`),
/// seen from `base_time` (Unix seconds). Only the ones that exist in the base (`exists`) and are not seeds. From the
/// strongest to the weakest: by the confidence (`by_lift` false) or by the confidence corrected by the file
/// frequency (`by_lift`).
fn neighbours(
    hist: &History,
    ancestors: &HashSet<String>,
    base_time: i64,
    seeds: &[String],
    exists: &HashSet<String>,
    half_life: Option<f64>,
    by_lift: bool,
) -> Vec<Neighbour> {
    let mut found: HashMap<&str, (f64, u32)> = HashMap::new();
    for seed in seeds.iter().filter(|s| !is_noise(s)) {
        let Some(hashes) = hist.by_file.get(seed) else { continue };
        let mut total = 0.0;
        let mut together: HashMap<&str, (f64, u32)> = HashMap::new();
        for hash in hashes.iter().filter(|h| ancestors.contains(*h)) {
            let commit = &hist.commits[hash];
            let w = weight((base_time - commit.time) as f64 / 86_400.0, half_life);
            total += w;
            for f in commit.files.iter().filter(|f| *f != seed) {
                let e = together.entry(f).or_default();
                e.0 += w;
                e.1 += 1;
            }
        }
        if total <= 0.0 {
            continue;
        }
        for (f, (w, n)) in together {
            let e = found.entry(f).or_default();
            e.0 += w / total;
            e.1 = e.1.max(n);
        }
    }
    let n_total = hist.commits.keys().filter(|h| ancestors.contains(*h)).count().max(1) as f64;
    let mut out: Vec<Neighbour> = found
        .into_iter()
        .filter(|(f, _)| exists.contains(*f) && !seeds.iter().any(|s| s == f))
        .map(|(f, (score, count))| {
            let n_file = hist.by_file.get(f).map_or(0, |hs| hs.iter().filter(|h| ancestors.contains(*h)).count()).max(1) as f64;
            Neighbour { path: f.to_owned(), score, count, lift: score / (n_file / n_total) }
        })
        .collect();
    out.sort_by(|a, b| {
        let first = if by_lift { b.lift.total_cmp(&a.lift) } else { b.score.total_cmp(&a.score) };
        first.then(b.score.total_cmp(&a.score)).then(b.count.cmp(&a.count)).then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Control of the co-change: the files that changed the most before the base, whatever the request (the "central
/// ones" of the repository). If suggesting those gets as much right as suggesting the neighbours, the co-change adds
/// nothing.
fn popular(hist: &History, ancestors: &HashSet<String>, exists: &HashSet<String>, seeds: &[String]) -> Vec<Neighbour> {
    let mut count: HashMap<&str, u32> = HashMap::new();
    for (hash, commit) in &hist.commits {
        if ancestors.contains(hash) {
            for f in &commit.files {
                *count.entry(f).or_default() += 1;
            }
        }
    }
    let mut out: Vec<Neighbour> = count
        .into_iter()
        .filter(|(f, _)| exists.contains(*f) && !seeds.iter().any(|s| s == f))
        .map(|(f, n)| Neighbour { path: f.to_owned(), score: n as f64, count: n, lift: 1.0 })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.path.cmp(&b.path)));
    out
}

/// The co-change neighbours of `p` (half-life `hi`, and whether ordered by lift), or the control of the most-changed
/// files (`None`).
fn source(p: &Prepared, key: Option<(usize, bool)>) -> &[Neighbour] {
    key.map_or(&p.popular, |(hi, lift)| &p.neigh[hi * 2 + usize::from(lift)])
}

/// The first `k` neighbours with at least `min_count` changes together and weight `min_score`.
fn pick(all: &[Neighbour], k: usize, min_count: u32, min_score: f64) -> Vec<&Neighbour> {
    all.iter().filter(|n| n.count >= min_count && n.score >= min_score).take(k).collect()
}

// ---------- Cases ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sample {
    Task,
    Commit,
}

struct Case {
    id: String,
    sample: Sample,
    /// The request: the `brief` of the task or the commit message.
    text: String,
    /// Where the agent started (the repository as it saw it).
    base: String,
    /// Changed files and whether they are new (they did not exist in the base).
    changed: Vec<(String, bool)>,
}

/// `git diff --name-status` (without renames) → (path, is new).
fn parse_name_status(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|l| {
            let (status, path) = l.split_once('\t')?;
            Some((path.trim().to_owned(), status.starts_with('A')))
        })
        .collect()
}

/// A case with everything the score needs, computed once.
struct Prepared {
    id: String,
    sample: Sample,
    learnings: Vec<Learning>,
    /// Paths cited in the request that exist in the base.
    seeds: Vec<String>,
    /// Learnings with `touches` that match what the case changed.
    truth: HashSet<String>,
    /// Files the case changed, already existing in the base and outside the noise (the prediction tries to hit them).
    actual: HashSet<String>,
    /// Neighbours of the co-change, one set per half-life of [`HALF_LIVES`].
    neigh: Vec<Vec<Neighbour>>, // per half-life and, inside it, [confidence, lift]
    /// The control: the most-changed files of the base, unrelated to the request.
    popular: Vec<Neighbour>,
}

impl Prepared {
    fn has_specific(&self) -> bool {
        self.learnings.iter().any(|l| !l.is_global())
    }

    fn file_seeds(&self) -> Vec<String> {
        self.seeds.iter().filter(|s| !is_noise(s)).cloned().collect()
    }
}

// ---------- Score ----------

/// What one strategy delivered: which learnings in full, which only as an index line, and the size.
struct Delivery {
    full: HashSet<String>,
    index: HashSet<String>,
    chars: usize,
}

/// Reads what `rendered` (the output of `learnings::render*`) delivered of each learning.
fn read_delivery(all: &[Learning], rendered: &str) -> Delivery {
    let mut d = Delivery { full: HashSet::new(), index: HashSet::new(), chars: rendered.chars().count() };
    for l in all {
        if rendered.contains(&format!("### {} ({})", l.titulo, l.file)) {
            d.full.insert(l.file.clone());
        } else if rendered.contains(&format!("- {} — ", l.file)) {
            d.index.insert(l.file.clone());
        }
    }
    d
}

/// Sum of the scores of one strategy in one sample. Only the cases that had a specific learning in the base go in.
#[derive(Default, Clone)]
struct MemTally {
    cases: usize,
    cases_with_truth: usize,
    truth: usize,
    hits: usize,
    /// Specific learnings delivered in full (the base of the precision).
    delivered: usize,
    /// Right ones that did not come in full but were there as an index line.
    in_index_only: usize,
    chars: usize,
    recall_sum: f64,
}

impl MemTally {
    fn add(&mut self, p: &Prepared, d: &Delivery) {
        self.cases += 1;
        self.chars += d.chars;
        let specific_full = d.full.iter().filter(|f| p.learnings.iter().any(|l| &l.file == *f && !l.is_global())).count();
        self.delivered += specific_full;
        let hits = p.truth.iter().filter(|t| d.full.contains(*t)).count();
        self.in_index_only += p.truth.iter().filter(|t| !d.full.contains(*t) && d.index.contains(*t)).count();
        self.hits += hits;
        if !p.truth.is_empty() {
            self.cases_with_truth += 1;
            self.truth += p.truth.len();
            self.recall_sum += hits as f64 / p.truth.len() as f64;
        }
    }

    fn recall(&self) -> f64 {
        ratio(self.hits, self.truth)
    }

    fn recall_macro(&self) -> f64 {
        if self.cases_with_truth == 0 {
            0.0
        } else {
            self.recall_sum / self.cases_with_truth as f64
        }
    }

    fn precision(&self) -> f64 {
        ratio(self.hits, self.delivered)
    }

    fn avg_chars(&self) -> f64 {
        if self.cases == 0 {
            0.0
        } else {
            self.chars as f64 / self.cases as f64
        }
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

/// Sum of the file prediction: the cited ones against the co-change neighbours.
#[derive(Default, Clone)]
struct FileTally {
    cases: usize,
    actual: usize,
    seeds: usize,
    seed_hits: usize,
    suggested: usize,
    suggested_hits: usize,
    /// Cases in which some neighbour was suggested, and the sum of their precision (average per case, without the
    /// weight of the huge cases).
    cases_suggested: usize,
    suggested_precision_sum: f64,
    /// Sum, per case, of the recall of the cited ones + neighbours.
    recall_sum: f64,
}

impl FileTally {
    fn add(&mut self, p: &Prepared, picked: &[&Neighbour]) {
        let seeds = p.file_seeds();
        let seed_hits = seeds.iter().filter(|s| p.actual.contains(*s)).count();
        let hits = picked.iter().filter(|n| p.actual.contains(&n.path)).count();
        self.cases += 1;
        self.actual += p.actual.len();
        self.seeds += seeds.len();
        self.seed_hits += seed_hits;
        self.suggested += picked.len();
        self.suggested_hits += hits;
        if !picked.is_empty() {
            self.cases_suggested += 1;
            self.suggested_precision_sum += hits as f64 / picked.len() as f64;
        }
        self.recall_sum += (seed_hits + hits) as f64 / p.actual.len().max(1) as f64;
    }

    fn suggested_precision_macro(&self) -> f64 {
        if self.cases_suggested == 0 {
            0.0
        } else {
            self.suggested_precision_sum / self.cases_suggested as f64
        }
    }

    fn recall_macro(&self) -> f64 {
        if self.cases == 0 {
            0.0
        } else {
            self.recall_sum / self.cases as f64
        }
    }
}

/// Header of the block of full texts, as in `learnings::render_with`.
const FULL_HEADER: &str = "Apply to this work (whole project or the files in question):\n\n";

/// Layered delivery, inside `budget`: first the learnings of the cited paths, then the ones of the co-change
/// neighbours (`extra`), and last the global ones. With an empty `extra` it gives the same as
/// `learnings::render_with(.., 0)`; the difference is that a neighbour never takes the place of a learning the
/// request itself already asked for.
fn layered(all: &[Learning], seeds: &[String], extra: &[String], budget: usize) -> String {
    let specific = |paths: &[String]| all.iter().filter(|l| !l.is_global() && l.touches_any(paths)).collect::<Vec<_>>();
    let mut seen: HashSet<&str> = HashSet::new();
    let (mut used, mut texts) = (0usize, Vec::new());
    for l in specific(seeds).into_iter().chain(specific(extra)).chain(all.iter().filter(|l| l.is_global())) {
        if !seen.insert(&l.file) {
            continue;
        }
        let text = l.full_text();
        let cost = text.chars().count();
        if used + cost <= budget {
            used += cost;
            texts.push(text);
        }
    }
    if texts.is_empty() {
        String::new()
    } else {
        format!("{FULL_HEADER}{}", texts.join("\n\n"))
    }
}

fn pct(x: f64) -> String {
    format!("{:>5.1}%", x * 100.0)
}

// ---------- Building the cases ----------

/// The learnings in the tree of `base`, as `learnings::load` would read them there. `blobs` keeps what was already
/// read.
fn learnings_at(repo: &Repo, tree: &[(String, String, u64)], blobs: &mut HashMap<(String, String), Option<Learning>>) -> Vec<Learning> {
    let prefix = format!("{}/", learnings::DIR);
    let mut out = Vec::new();
    for (path, sha, size) in tree {
        let Some(name) = path.strip_prefix(&prefix) else { continue };
        if name.contains('/') || !name.to_ascii_lowercase().ends_with(".md") || name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        if *size > MAX_LEARNING_BYTES {
            continue;
        }
        let learning = blobs
            .entry((path.clone(), sha.clone()))
            .or_insert_with(|| {
                let raw = repo.bytes(&["cat-file", "blob", sha]).ok()?;
                learnings::parse(path, &holt_core::util::decode_text(&raw)).ok()
            })
            .clone();
        out.extend(learning);
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// `git ls-tree -r -l`: (path, blob, size).
fn parse_tree(text: &str) -> Vec<(String, String, u64)> {
    text.lines()
        .filter_map(|l| {
            let (meta, path) = l.split_once('\t')?;
            let mut f = meta.split_whitespace();
            let (_mode, kind, sha, size) = (f.next()?, f.next()?, f.next()?, f.next()?);
            (kind == "blob").then(|| (path.to_owned(), sha.to_owned(), size.parse().unwrap_or(0)))
        })
        .collect()
}

struct Ctx<'a> {
    repo: &'a Repo,
    hist: &'a History,
    blobs: HashMap<(String, String), Option<Learning>>,
}

impl Ctx<'_> {
    fn prepare(&mut self, case: &Case) -> Result<Prepared, String> {
        let repo = self.repo;
        let tree = parse_tree(&repo.text(&["ls-tree", "-r", "-l", &case.base])?);
        let exists: HashSet<String> = tree.iter().map(|(p, _, _)| p.clone()).collect();
        let all = learnings_at(repo, &tree, &mut self.blobs);
        let seeds = learnings::paths_in_text_where(&case.text, |p| exists.contains(p));
        let changed_paths: Vec<String> = case.changed.iter().map(|(p, _)| learnings::normalize_path(p)).collect();
        let truth: HashSet<String> =
            all.iter().filter(|l| !l.is_global() && l.touches_any(&changed_paths)).map(|l| l.file.clone()).collect();
        let actual: HashSet<String> =
            case.changed.iter().filter(|(p, added)| !added && !is_noise(p) && exists.contains(p)).map(|(p, _)| p.clone()).collect();
        let ancestors: HashSet<String> = repo.text(&["rev-list", &case.base])?.lines().map(str::to_owned).collect();
        let base_time: i64 = repo.text(&["log", "-1", "--format=%ct", &case.base])?.trim().parse().map_err(|_| "base date".to_owned())?;
        let file_seeds: Vec<String> = seeds.iter().filter(|s| !is_noise(s)).cloned().collect();
        let neigh = HALF_LIVES
            .iter()
            .flat_map(|h| [false, true].map(|lift| neighbours(self.hist, &ancestors, base_time, &file_seeds, &exists, *h, lift)))
            .collect();
        let popular = popular(self.hist, &ancestors, &exists, &file_seeds);
        Ok(Prepared { id: case.id.clone(), sample: case.sample, learnings: all, seeds, truth, actual, neigh, popular })
    }
}

/// The "Merge task tN: …" merges (or "Merge tarefa tN: …", before the English rename) of the history: task id → (merge, parent 1, parent 2).
fn task_merges(repo: &Repo) -> Result<HashMap<String, (String, String, String)>, String> {
    let log = repo.text(&["log", "--merges", "--format=%H\t%P\t%s", "HEAD"])?;
    let mut out = HashMap::new();
    for line in log.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(hash), Some(parents), Some(subject)) = (f.next(), f.next(), f.next()) else { continue };
        let Some(rest) = subject.strip_prefix("Merge task ").or_else(|| subject.strip_prefix("Merge tarefa ")) else { continue };
        let Some((id, _)) = rest.split_once(':') else { continue };
        let ps: Vec<&str> = parents.split_whitespace().collect();
        if ps.len() == 2 {
            out.entry(id.trim().to_owned()).or_insert((hash.to_owned(), ps[0].to_owned(), ps[1].to_owned()));
        }
    }
    Ok(out)
}

/// Where the task started: the parent of the oldest commit of the branch's own line (the first parent of each commit).
fn branch_point(repo: &Repo, p1: &str, p2: &str) -> Option<String> {
    let range = format!("{p1}..{p2}");
    let own = repo.text(&["rev-list", "--first-parent", &range]).ok()?;
    let oldest = own.lines().last()?.to_owned();
    Some(repo.text(&["rev-parse", &format!("{oldest}^")]).ok()?.trim().to_owned())
}

fn task_cases(repo: &Repo, tasks_json: &str) -> Result<Vec<Case>, String> {
    let json: serde_json::Value = serde_json::from_str(tasks_json).map_err(|e| format!("tasks file: {e}"))?;
    let merges = task_merges(repo)?;
    let mut out = Vec::new();
    for t in json["tasks"].as_array().ok_or("the tasks file has no \"tasks\" list")? {
        if t["status"] != "merged" {
            continue;
        }
        let (Some(id), Some(brief)) = (t["id"].as_str(), t["brief"].as_str()) else { continue };
        let Some((merge, p1, p2)) = merges.get(id) else {
            eprintln!("{id}: no merge commit in the history, out of the sample");
            continue;
        };
        let Some(base) = branch_point(repo, p1, p2) else {
            eprintln!("{id}: could not find the base, out of the sample");
            continue;
        };
        let changed = parse_name_status(&repo.text(&["diff", "--no-renames", "--name-status", p1, merge])?);
        out.push(Case { id: id.to_owned(), sample: Sample::Task, text: brief.to_owned(), base, changed });
    }
    Ok(out)
}

fn commit_cases(repo: &Repo, min_files: usize) -> Result<Vec<Case>, String> {
    let mut out = Vec::new();
    for hash in repo.text(&["log", "--first-parent", "--no-merges", "--format=%H", "HEAD"])?.lines() {
        let Ok(parent) = repo.text(&["rev-parse", &format!("{hash}^")]) else { continue };
        let changed = parse_name_status(&repo.text(&["diff-tree", "--no-commit-id", "--no-renames", "--name-status", "-r", hash])?);
        if changed.iter().filter(|(p, _)| !is_noise(p)).count() < min_files {
            continue;
        }
        let text = repo.text(&["log", "-1", "--format=%B", hash])?;
        out.push(Case { id: hash[..7].to_owned(), sample: Sample::Commit, text, base: parent.trim().to_owned(), changed });
    }
    Ok(out)
}

// ---------- Report ----------

fn sample_name(s: Sample) -> &'static str {
    match s {
        Sample::Task => "MERGED TASKS",
        Sample::Commit => "DIRECT COMMITS OF dev",
    }
}

fn report(cases: &[Prepared], sample: Sample, detail: bool) {
    let pop: Vec<&Prepared> = cases.iter().filter(|p| p.sample == sample).collect();
    println!("\n==================== {} ====================", sample_name(sample));
    let with_seed = pop.iter().filter(|p| !p.seeds.is_empty()).count();
    let with_learn = pop.iter().filter(|p| p.has_specific()).count();
    let with_truth = pop.iter().filter(|p| !p.truth.is_empty()).count();
    println!("cases: {}", pop.len());
    println!("seed (the request cites at least one path that exists in the repository): {} of {}", with_seed, pop.len());
    println!("with a specific learning in the base: {with_learn}; with an answer key (some learning matches what changed): {with_truth}");
    println!(
        "The score uses only the {with_learn} cases with a learning in the base; the recall only the {with_truth} with an answer key."
    );
    let scored: Vec<&Prepared> = pop.iter().copied().filter(|p| p.has_specific()).collect();
    if scored.is_empty() {
        return;
    }

    // Strategies (a), (a+real index), (b) and (c)
    let (mut a, mut a_real, mut b, mut all_full) = (MemTally::default(), MemTally::default(), MemTally::default(), MemTally::default());
    let mut a_free = MemTally::default();
    for p in &scored {
        let seeds = &p.seeds;
        a.add(p, &read_delivery(&p.learnings, &learnings::render_with(&p.learnings, seeds, BUDGET, 0)));
        a_real.add(p, &read_delivery(&p.learnings, &learnings::render(&p.learnings, seeds, BUDGET)));
        b.add(p, &read_delivery(&p.learnings, &learnings::render_with(&p.learnings, seeds, BUDGET, usize::MAX)));
        a_free.add(p, &read_delivery(&p.learnings, &layered(&p.learnings, seeds, &[], usize::MAX)));
        let texts: Vec<String> = p.learnings.iter().map(Learning::full_text).collect();
        let everything = format!("{FULL_HEADER}{}", texts.join("\n\n"));
        all_full.add(p, &read_delivery(&p.learnings, &everything));
    }
    println!("\n--- Memory: what each strategy delivers (cap of {BUDGET} characters of full texts) ---");
    println!("{:<46} {:>8} {:>9} {:>9} {:>9} {:>9}", "strategy", "recall", "recall*", "precision", "chars", "tokens");
    let line = |name: &str, t: &MemTally| {
        println!(
            "{:<46} {:>8} {:>9} {:>9} {:>9.0} {:>9.0}",
            name,
            pct(t.recall()),
            pct(t.recall_macro()),
            pct(t.precision()),
            t.avg_chars(),
            t.avg_chars() / 4.0
        )
    };
    line("(a) today, only the texts (no index)", &a);
    line("(a') today for real (index cut at 2500)", &a_real);
    line("(b) today + index of all the others", &b);
    line("    reference: (a) with no character cap", &a_free);
    line("    reference: every specific one in full", &all_full);
    for (name, t) in [("(a') today for real", &a_real), ("(b)", &b)] {
        println!(
            "  {name}: {} of the {} right ones that (a) did not deliver in full were there as an index line (the agent would have to ask with --learn).",
            t.in_index_only,
            t.truth - t.hits
        );
    }
    println!("  recall = right ones delivered in full ÷ right ones (all cases together); recall* = average of the recall of each case;");
    println!("  precision = right ones ÷ specific ones delivered in full. Size = average per case, with the global ones and the index.");

    // The character cap: what it costs and what raising it gains (strategy (a), without index).
    println!("\n--- Effect of the character cap in (a) ---");
    println!("{:<12} {:>8} {:>9} {:>9} {:>9}", "cap", "recall", "precision", "chars", "tokens");
    for cap in [3_000, BUDGET, 9_000, 12_000, usize::MAX] {
        let mut t = MemTally::default();
        for p in &scored {
            t.add(p, &read_delivery(&p.learnings, &learnings::render_with(&p.learnings, &p.seeds, cap, 0)));
        }
        let name = if cap == usize::MAX { "no cap".to_owned() } else { cap.to_string() };
        println!("{:<12} {:>8} {:>9} {:>9.0} {:>9.0}", name, pct(t.recall()), pct(t.precision()), t.avg_chars(), t.avg_chars() / 4.0);
    }

    // Co-change grid
    let file_pop: Vec<&Prepared> = pop.iter().copied().filter(|p| !p.file_seeds().is_empty() && !p.actual.is_empty()).collect();
    let mut grid: Vec<Setting> = Vec::new();
    for (hi, lift) in (0..HALF_LIVES.len()).flat_map(|hi| [(hi, false), (hi, true)]) {
        for k in KS {
            for min_count in MIN_COUNTS {
                for min_score in MIN_SCORES {
                    grid.push((Some((hi, lift)), k, min_count, min_score));
                }
            }
        }
    }
    // Control: the most-changed files, without looking at the request.
    grid.extend(KS.iter().map(|&k| (None, k, 1, 0.0)));
    let label = |key: Option<(usize, bool)>, k: usize, min_count: u32, min_score: f64| {
        let name = key.map_or("ctrl".to_owned(), |(h, lift)| {
            let half = HALF_LIVES[h].map_or("none".to_owned(), |d| format!("{d}d"));
            format!("{half}/{}", if lift { "lift" } else { "conf" })
        });
        format!("{name:<8} {k:>4} {min_count:>5} {min_score:>5.1}")
    };
    println!("\n--- (c) co-change for the memory: grid of thresholds ({} cases) ---", scored.len());
    println!("ctrl = control: the files that changed the most before the base, without looking at the request.");
    println!(
        "decay = half-life in days; nei = at most how many neighbours; times = changed together at least; weight = sum of the confidences."
    );
    println!("Layers: the learnings of the cited ones first, then the ones of the neighbours, inside the cap. w/o cap = the same without the cap.");
    println!(
        "{:<8} {:>4} {:>5} {:>5} | {:>8} {:>9} {:>8} | {:>10} {:>11} {:>10}",
        "decay", "nei", "times", "weight", "recall", "precision", "tokens", "rec.w/o cap", "prec.w/o cap", "tok.w/o cap"
    );
    for &(key, k, min_count, min_score) in &grid {
        let (mut c, mut free) = (MemTally::default(), MemTally::default());
        for p in &scored {
            let extra: Vec<String> = pick(source(p, key), k, min_count, min_score).iter().map(|n| n.path.clone()).collect();
            c.add(p, &read_delivery(&p.learnings, &layered(&p.learnings, &p.seeds, &extra, BUDGET)));
            free.add(p, &read_delivery(&p.learnings, &layered(&p.learnings, &p.seeds, &extra, usize::MAX)));
        }
        println!(
            "{} | {:>8} {:>9} {:>8.0} | {:>10} {:>11} {:>10.0}",
            label(key, k, min_count, min_score),
            pct(c.recall()),
            pct(c.precision()),
            c.avg_chars() / 4.0,
            pct(free.recall()),
            pct(free.precision()),
            free.avg_chars() / 4.0
        );
    }

    println!("\n--- File prediction by the co-change: grid of thresholds ({} cases with a seed and changed files) ---", file_pop.len());
    println!(
        "suggested = neighbours per case; prec.nei = neighbours the case actually changed ÷ neighbours (prec.nei* = average per case);"
    );
    println!("rec.all / prec.all = cited ones + neighbours against the changed files (rec.all* = average per case).");
    println!(
        "{:<8} {:>4} {:>5} {:>5} | {:>9} {:>8} {:>9} | {:>8} {:>9} {:>9}",
        "decay", "nei", "times", "weight", "suggested", "prec.nei", "prec.nei*", "rec.all", "rec.all*", "prec.all"
    );
    let mut seeds_only = FileTally::default();
    for p in &file_pop {
        seeds_only.add(p, &[]);
    }
    println!(
        "only the cited ones (no co-change): precision {} recall {} recall* {} ({} cited files, {} changed)",
        pct(ratio(seeds_only.seed_hits, seeds_only.seeds)),
        pct(ratio(seeds_only.seed_hits, seeds_only.actual)),
        pct(seeds_only.recall_macro()),
        seeds_only.seeds,
        seeds_only.actual
    );
    for &(key, k, min_count, min_score) in &grid {
        let mut f = FileTally::default();
        for p in &file_pop {
            f.add(p, &pick(source(p, key), k, min_count, min_score));
        }
        println!(
            "{} | {:>9.2} {:>8} {:>9} | {:>8} {:>9} {:>9}",
            label(key, k, min_count, min_score),
            ratio(f.suggested, f.cases),
            pct(ratio(f.suggested_hits, f.suggested)),
            pct(f.suggested_precision_macro()),
            pct(ratio(f.seed_hits + f.suggested_hits, f.actual)),
            pct(f.recall_macro()),
            pct(ratio(f.seed_hits + f.suggested_hits, f.seeds + f.suggested)),
        );
    }

    if detail {
        println!("\n--- Per case ---");
        for p in &pop {
            let d = read_delivery(&p.learnings, &learnings::render_with(&p.learnings, &p.seeds, BUDGET, 0));
            let got: Vec<&String> = p.truth.iter().filter(|t| d.full.contains(*t)).collect();
            println!(
                "{:<8} seeds {:>2} | learnings in the base {:>2} | right ones {:>2} | (a) got {:>2} | changed files {:>3}",
                p.id,
                p.seeds.len(),
                p.learnings.len(),
                p.truth.len(),
                got.len(),
                p.actual.len()
            );
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let detail = take_flag(&mut args, "--detalhe");
    let min_files = take_value(&mut args, "--commits-min-arquivos").and_then(|v| v.parse().ok()).unwrap_or(3);
    if args.len() != 2 {
        eprintln!("usage: memeval <repository> <tasks file> [--commits-min-arquivos N] [--detalhe]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args[0], &args[1], min_files, detail) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.remove(at);
    (at < args.len()).then(|| args.remove(at))
}

fn run(repo: &str, tasks: &str, min_files: usize, detail: bool) -> Result<(), String> {
    let repo = Repo { dir: PathBuf::from(repo) };
    let tasks_json = std::fs::read(tasks).map_err(|e| format!("{tasks}: {e}"))?;
    let tasks_json = holt_core::util::decode_text(&tasks_json);
    let hist = parse_history(
        &repo.text(&["log", "--no-merges", "--name-only", "--format=%x01%H %ct", "HEAD"]).map_err(|e| format!("history: {e}"))?,
    );
    eprintln!("history: {} commits useful for the co-change", hist.commits.len());
    let mut cases = task_cases(&repo, &tasks_json)?;
    cases.extend(commit_cases(&repo, min_files)?);
    eprintln!("{} cases; preparing…", cases.len());
    let mut ctx = Ctx { repo: &repo, hist: &hist, blobs: HashMap::new() };
    let mut prepared = Vec::new();
    for case in &cases {
        match ctx.prepare(case) {
            Ok(p) => prepared.push(p),
            Err(e) => eprintln!("{}: {e}", case.id),
        }
    }
    report(&prepared, Sample::Task, detail);
    report(&prepared, Sample::Commit, detail);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learning(file: &str, touches: &[&str]) -> Learning {
        Learning {
            file: file.into(),
            titulo: format!("T {file}"),
            resumo: "r".into(),
            touches: touches.iter().map(|s| s.to_string()).collect(),
            body: "body".into(),
        }
    }

    fn prepared(learnings: Vec<Learning>, truth: &[&str]) -> Prepared {
        Prepared {
            id: "x".into(),
            sample: Sample::Task,
            learnings,
            seeds: vec![],
            truth: truth.iter().map(|s| s.to_string()).collect(),
            actual: HashSet::new(),
            neigh: vec![],
            popular: vec![],
        }
    }

    const LOG: &str =
        "\u{1}aaa 1000\n\nsrc/a.rs\nsrc/b.rs\nCargo.lock\nmemory/x.md\n\u{1}bbb 2000\n\nsrc/a.rs\nsrc/c.rs\n\u{1}ccc 3000\n\nmemory/y.md\n";

    #[test]
    fn history_drops_noise_and_commits_with_only_noise() {
        let h = parse_history(LOG);
        assert_eq!(h.commits.len(), 2);
        assert_eq!(h.commits["aaa"].files, ["src/a.rs", "src/b.rs"]);
        assert_eq!(h.by_file["src/a.rs"], ["aaa", "bbb"]);
    }

    #[test]
    fn history_skips_big_commits() {
        let files: String = (0..=MAX_COMMIT_FILES).map(|i| format!("f{i}.rs\n")).collect();
        assert!(parse_history(&format!("\u{1}big 1\n\n{files}")).commits.is_empty());
        let files: String = (0..MAX_COMMIT_FILES).map(|i| format!("f{i}.rs\n")).collect();
        assert_eq!(parse_history(&format!("\u{1}ok 1\n\n{files}")).commits.len(), 1);
    }

    fn setup() -> (History, HashSet<String>, HashSet<String>) {
        let ancestors = ["aaa", "bbb"].iter().map(|s| s.to_string()).collect();
        let exists = ["src/a.rs", "src/b.rs", "src/c.rs"].iter().map(|s| s.to_string()).collect();
        (parse_history(LOG), ancestors, exists)
    }

    #[test]
    fn neighbours_count_and_rank_by_confidence() {
        let (h, anc, exists) = setup();
        let n = neighbours(&h, &anc, 2000, &["src/a.rs".to_owned()], &exists, None, false);
        // a changed in 2 commits: b together in 1, c together in 1 → confidence 0.5 each, tie broken by the path.
        assert_eq!(n.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["src/b.rs", "src/c.rs"]);
        assert!(n.iter().all(|n| n.count == 1 && (n.score - 0.5).abs() < 1e-9));
    }

    #[test]
    fn neighbours_ignore_commits_after_the_base_and_files_that_do_not_exist() {
        let (h, mut anc, mut exists) = setup();
        anc.remove("bbb");
        let n = neighbours(&h, &anc, 2000, &["src/a.rs".to_owned()], &exists, None, false);
        assert_eq!(n.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["src/b.rs"]);
        assert!((n[0].score - 1.0).abs() < 1e-9);
        exists.remove("src/b.rs");
        assert!(neighbours(&h, &anc, 2000, &["src/a.rs".to_owned()], &exists, None, false).is_empty());
    }

    #[test]
    fn decay_favours_recent_commits() {
        let (_, anc, exists) = setup();
        // aaa (with b) is 7 days older than bbb (with c); the base comes right after bbb.
        let h = parse_history(&LOG.replace("bbb 2000", &format!("bbb {}", 1000 + 7 * 86_400)));
        let base = 1000 + 7 * 86_400;
        let seeds = ["src/a.rs".to_owned()];
        let flat = neighbours(&h, &anc, base, &seeds, &exists, None, false);
        assert!((flat[0].score - flat[1].score).abs() < 1e-9);
        let decayed = neighbours(&h, &anc, base, &seeds, &exists, Some(7.0), false);
        assert_eq!(decayed.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["src/c.rs", "src/b.rs"]);
        assert!((decayed[0].score - 2.0 / 3.0).abs() < 1e-9);
        assert!((weight(7.0, Some(7.0)) - 0.5).abs() < 1e-9 && weight(100.0, None) == 1.0 && weight(-3.0, Some(7.0)) == 1.0);
    }

    #[test]
    fn neighbours_exclude_seeds_and_noise_seeds() {
        let (h, anc, exists) = setup();
        let seeds = ["src/a.rs".to_owned(), "src/b.rs".to_owned(), "memory/x.md".to_owned()];
        let n = neighbours(&h, &anc, 2000, &seeds, &exists, None, false);
        assert_eq!(n.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["src/c.rs"]);
    }

    #[test]
    fn pick_applies_thresholds_and_limit() {
        let all: Vec<Neighbour> = [("a", 0.9, 3), ("b", 0.5, 1), ("c", 0.4, 2)]
            .iter()
            .map(|(p, s, c)| Neighbour { path: p.to_string(), score: *s, count: *c, lift: 1.0 })
            .collect();
        assert_eq!(pick(&all, 10, 1, 0.0).len(), 3);
        assert_eq!(pick(&all, 10, 2, 0.0).iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(pick(&all, 10, 1, 0.45).iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(pick(&all, 1, 1, 0.0).len(), 1);
    }

    #[test]
    fn name_status_marks_new_files() {
        let v = parse_name_status("M\tsrc/a.rs\nA\tsrc/new.rs\nD\told.rs\n");
        assert_eq!(v, [("src/a.rs".to_owned(), false), ("src/new.rs".to_owned(), true), ("old.rs".to_owned(), false)]);
    }

    #[test]
    fn tree_keeps_only_blobs() {
        let t = parse_tree("100644 blob abc123      12\tcrates/a.rs\n160000 commit def456       -\tsub\n");
        assert_eq!(t, [("crates/a.rs".to_owned(), "abc123".to_owned(), 12)]);
    }

    #[test]
    fn delivery_tells_full_text_from_index_line() {
        let all = vec![learning("memory/learnings/a.md", &["src/a.rs"]), learning("memory/learnings/b.md", &["src/b.rs"])];
        let rendered = learnings::render_with(&all, &["src/a.rs".to_owned()], BUDGET, usize::MAX);
        let d = read_delivery(&all, &rendered);
        assert_eq!(d.full, ["memory/learnings/a.md".to_owned()].into_iter().collect());
        assert_eq!(d.index, ["memory/learnings/b.md".to_owned()].into_iter().collect());
        assert_eq!(d.chars, rendered.chars().count());
        let d = read_delivery(&all, &learnings::render_with(&all, &["src/a.rs".to_owned()], BUDGET, 0));
        assert!(d.index.is_empty());
    }

    #[test]
    fn tally_scores_recall_precision_and_index() {
        let all = vec![
            learning("memory/learnings/a.md", &["src/a.rs"]),
            learning("memory/learnings/b.md", &["src/b.rs"]),
            learning("memory/learnings/c.md", &["src/c.rs"]),
        ];
        // The right ones are a and b; the guess delivers a (right) and c (wrong); b only shows up in the index.
        let p = prepared(all.clone(), &["memory/learnings/a.md", "memory/learnings/b.md"]);
        let rendered = learnings::render_with(&all, &["src/a.rs".to_owned(), "src/c.rs".to_owned()], BUDGET, usize::MAX);
        let mut t = MemTally::default();
        t.add(&p, &read_delivery(&all, &rendered));
        assert_eq!((t.hits, t.truth, t.delivered, t.in_index_only), (1, 2, 2, 1));
        assert!((t.recall() - 0.5).abs() < 1e-9 && (t.precision() - 0.5).abs() < 1e-9 && (t.recall_macro() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tally_ignores_cases_without_truth_in_recall_but_counts_their_delivery() {
        let all = vec![learning("memory/learnings/a.md", &["src/a.rs"])];
        let p = prepared(all.clone(), &[]);
        let mut t = MemTally::default();
        t.add(&p, &read_delivery(&all, &learnings::render_with(&all, &["src/a.rs".to_owned()], BUDGET, 0)));
        assert_eq!((t.cases, t.cases_with_truth, t.delivered, t.hits), (1, 0, 1, 0));
        assert_eq!(t.recall_macro(), 0.0);
    }

    #[test]
    fn file_tally_separates_cited_from_suggested() {
        let mut p = prepared(vec![], &[]);
        p.seeds = vec!["src/a.rs".into(), "memory/x.md".into(), "src/z.rs".into()];
        p.actual = ["src/a.rs", "src/b.rs"].iter().map(|s| s.to_string()).collect();
        let n = [
            Neighbour { path: "src/b.rs".into(), score: 1.0, count: 2, lift: 1.0 },
            Neighbour { path: "src/q.rs".into(), score: 0.5, count: 1, lift: 1.0 },
        ];
        let mut f = FileTally::default();
        f.add(&p, &n.iter().collect::<Vec<_>>());
        assert_eq!((f.seeds, f.seed_hits, f.suggested, f.suggested_hits, f.actual), (2, 1, 2, 1, 2));
    }

    #[test]
    fn layered_matches_render_without_extras_and_puts_seed_learnings_first() {
        let mut all = vec![learning("memory/learnings/a.md", &["src/a.rs"]), learning("memory/learnings/b.md", &["src/b.rs"])];
        all.push(learning("memory/learnings/g.md", &[]));
        let seeds = vec!["src/b.rs".to_owned()];
        assert_eq!(layered(&all, &seeds, &[], BUDGET), learnings::render_with(&all, &seeds, BUDGET, 0));
        // The cap fits only one text: the one of the cited file (b) is worth more than the one of the neighbour (a),
        // even though a comes first in the folder.
        let one = all[1].full_text().chars().count();
        let rendered = layered(&all, &seeds, &["src/a.rs".to_owned()], one);
        assert!(rendered.contains(&all[1].full_text()) && !rendered.contains(&all[0].full_text()));
        // Without a cap, both come in, plus the global one, once each.
        let rendered = layered(&all, &seeds, &["src/a.rs".to_owned(), "src/b.rs".to_owned()], usize::MAX);
        assert_eq!(rendered.matches("### ").count(), 3);
        assert_eq!(layered(&[], &seeds, &[], BUDGET), "");
    }

    #[test]
    fn lift_ranks_specific_neighbours_above_hub_files() {
        let log = "\u{1}c1 1\n\na.rs\nhub.rs\nrare.rs\n\u{1}c2 2\n\na.rs\nhub.rs\n\u{1}c3 3\n\nhub.rs\n\u{1}c4 4\n\nhub.rs\n\u{1}c5 5\n\nhub.rs\n\u{1}c6 6\n\nhub.rs\n";
        let h = parse_history(log);
        let anc: HashSet<String> = h.commits.keys().cloned().collect();
        let exists: HashSet<String> = ["a.rs", "hub.rs", "rare.rs"].iter().map(|s| s.to_string()).collect();
        let seeds = ["a.rs".to_owned()];
        let by_conf = neighbours(&h, &anc, 6, &seeds, &exists, None, false);
        assert_eq!(by_conf.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["hub.rs", "rare.rs"]);
        let by_lift = neighbours(&h, &anc, 6, &seeds, &exists, None, true);
        assert_eq!(by_lift.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["rare.rs", "hub.rs"]);
        assert!((by_lift[0].lift - 3.0).abs() < 1e-9 && (by_lift[1].lift - 1.0).abs() < 1e-9);
    }

    #[test]
    fn popular_ranks_by_frequency_and_skips_seeds() {
        let (h, anc, exists) = setup();
        let p = popular(&h, &anc, &exists, &["src/b.rs".to_owned()]);
        assert_eq!(p.iter().map(|n| (n.path.as_str(), n.count)).collect::<Vec<_>>(), [("src/a.rs", 2), ("src/c.rs", 1)]);
    }
    #[test]
    fn noise_is_memory_and_cargo_lock() {
        assert!(is_noise("memory/OVERVIEW.md") && is_noise("Cargo.lock"));
        assert!(!is_noise("crates/core/Cargo.lock.rs") && !is_noise("src/memory/x.rs"));
    }
}
