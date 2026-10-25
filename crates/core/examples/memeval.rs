//! Régua da memória: mede o que cada jeito de entregar aprendizados teria acertado nas tarefas já mescladas.
//!
//! ```text
//! cargo run -p agentdeck-core --example memeval -- <repositório> <arquivo de tarefas> [opções]
//! ```
//!
//! O `<arquivo de tarefas>` é o `dados/tarefas/<projeto>.json` do AgentDeck. Só leitura: o programa chama o `git`
//! instalado e imprime tabelas; nada é gravado.
//!
//! Para cada caso (uma tarefa mesclada, ou um commit direto da `dev` com diff grande o bastante):
//!
//! - **O certo:** os aprendizados (com `touches`) que casam com os arquivos que o caso de fato mudou, lidos **como
//!   estavam na base** do caso, para a resposta não vazar do futuro. Os globais ficam fora da conta (chegam sempre).
//! - **O palpite:** o que cada estratégia entregaria sabendo só o pedido (o `brief`, ou a mensagem do commit):
//!   (a) hoje: os aprendizados dos caminhos citados no pedido; (b) hoje + o índice de uma linha de todos os outros;
//!   (c) co-mudança: os caminhos citados, ampliados pelos arquivos que mudavam junto com eles antes da base.
//! - **A nota:** recall, precisão e tamanho entregue (caracteres e tokens, caracteres ÷ 4).
//!
//! Também mede a previsão de arquivos (os citados + os vizinhos da co-mudança contra os que o caso mudou) e a
//! semente (quantos pedidos citam algum caminho do repositório).
//!
//! Opções: `--commits-min-arquivos N` (padrão 3: tamanho mínimo, sem `memory/**` e `Cargo.lock`, dos commits da
//! segunda amostra), `--detalhe` (uma linha por caso).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;

use agentdeck_core::learnings::{self, Learning};

/// Commits com mais arquivos que isso (reformatações, renomeações) não ensinam co-mudança.
const MAX_COMMIT_FILES: usize = 15;
/// Orçamento de caracteres de aprendizados por inteiro, o mesmo da entrega real no início da tarefa.
const BUDGET: usize = learnings::START_BUDGET;
/// Os mesmos limites com que `learnings::load` lê um aprendizado.
const MAX_LEARNING_BYTES: u64 = 64 * 1024;
/// Meias-vidas (em dias) testadas para o decaimento pela idade; `None` = sem decaimento.
const HALF_LIVES: [Option<f64>; 3] = [None, Some(7.0), Some(3.0)];
const KS: [usize; 3] = [3, 5, 10];
const MIN_COUNTS: [u32; 3] = [1, 2, 3];
const MIN_SCORES: [f64; 2] = [0.0, 0.3];

/// Uma linha da grade: de onde vêm os vizinhos (meia-vida e ordenação por lift, ou `None` = o controle), quantos, o
/// mínimo de vezes juntos e o peso mínimo.
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
            .map_err(|e| format!("não consegui rodar o git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(out.stdout)
    }

    fn text(&self, args: &[&str]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.bytes(args)?).into_owned())
    }
}

// ---------- Histórico e co-mudança ----------

/// Arquivos que não entram na co-mudança nem na previsão: a memória e o que o cargo regrava sozinho.
fn is_noise(path: &str) -> bool {
    path.starts_with("memory/") || path == "Cargo.lock"
}

struct Commit {
    time: i64,
    files: Vec<String>,
}

/// Os commits (sem merges) que servem à co-mudança, por hash, e o índice arquivo → commits.
#[derive(Default)]
struct History {
    commits: HashMap<String, Commit>,
    by_file: HashMap<String, Vec<String>>,
}

/// Lê a saída de `git log --no-merges --name-only --format=%x01%H %ct`. Fica de fora o commit com mais de
/// [`MAX_COMMIT_FILES`] arquivos, e os arquivos de ruído; o commit que só tinha ruído some.
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

/// Peso de um commit com `age_days` de idade: 1 sem decaimento, ou cai pela metade a cada `half_life` dias.
fn weight(age_days: f64, half_life: Option<f64>) -> f64 {
    match half_life {
        None => 1.0,
        Some(h) => 0.5f64.powf(age_days.max(0.0) / h),
    }
}

/// Um arquivo que costuma mudar junto com as sementes.
#[derive(Debug, Clone, PartialEq)]
struct Neighbour {
    path: String,
    /// Soma, sobre as sementes, de (peso dos commits em que mudaram juntos ÷ peso dos commits da semente).
    score: f64,
    /// Quantas vezes mudaram juntos, na semente em que isso mais se repete.
    count: u32,
    /// A confiança dividida pela frequência do arquivo em todos os commits: quanto mais específico da semente, maior.
    lift: f64,
}

/// Os arquivos que mudaram junto com as `seeds` nos commits que são ancestrais da base (`ancestors`), vistos de
/// `base_time` (segundos Unix). Só os que existem na base (`exists`) e não são sementes. Do mais para o menos forte:
/// pela confiança (`by_lift` falso) ou pela confiança corrigida pela frequência do arquivo (`by_lift`).
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

/// Controle da co-mudança: os arquivos que mais mudaram antes da base, qualquer que seja o pedido (os "centrais" do
/// repositório). Se sugerir esses acertar tanto quanto sugerir os vizinhos, a co-mudança não acrescenta nada.
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

/// Os vizinhos de co-mudança de `p` (meia-vida `hi`, e se ordenados por lift), ou o controle dos mais mudados (`None`).
fn source(p: &Prepared, key: Option<(usize, bool)>) -> &[Neighbour] {
    key.map_or(&p.popular, |(hi, lift)| &p.neigh[hi * 2 + usize::from(lift)])
}

/// Os `k` primeiros vizinhos com pelo menos `min_count` mudanças juntas e peso `min_score`.
fn pick(all: &[Neighbour], k: usize, min_count: u32, min_score: f64) -> Vec<&Neighbour> {
    all.iter().filter(|n| n.count >= min_count && n.score >= min_score).take(k).collect()
}

// ---------- Casos ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sample {
    Task,
    Commit,
}

struct Case {
    id: String,
    sample: Sample,
    /// O pedido: o `brief` da tarefa ou a mensagem do commit.
    text: String,
    /// Onde o agente começou (o repositório como ele o via).
    base: String,
    /// Arquivos mudados e se são novos (não existiam na base).
    changed: Vec<(String, bool)>,
}

/// `git diff --name-status` (sem renomeações) → (caminho, é novo).
fn parse_name_status(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|l| {
            let (status, path) = l.split_once('\t')?;
            Some((path.trim().to_owned(), status.starts_with('A')))
        })
        .collect()
}

/// Um caso com tudo o que a nota precisa, calculado uma vez.
struct Prepared {
    id: String,
    sample: Sample,
    learnings: Vec<Learning>,
    /// Caminhos citados no pedido que existem na base.
    seeds: Vec<String>,
    /// Aprendizados com `touches` que casam com o que o caso mudou.
    truth: HashSet<String>,
    /// Arquivos que o caso mudou, já existentes na base e fora do ruído (a previsão tenta acertá-los).
    actual: HashSet<String>,
    /// Vizinhos da co-mudança, um conjunto por meia-vida de [`HALF_LIVES`].
    neigh: Vec<Vec<Neighbour>>, // por meia-vida e, dentro dela, [confiança, lift]
    /// O controle: os arquivos mais mudados da base, sem relação com o pedido.
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

// ---------- Nota ----------

/// O que uma estratégia entregou: quais aprendizados por inteiro, quais só por linha de índice, e o tamanho.
struct Delivery {
    full: HashSet<String>,
    index: HashSet<String>,
    chars: usize,
}

/// Lê o que `rendered` (a saída de `learnings::render*`) entregou de cada aprendizado.
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

/// Soma das notas de uma estratégia numa amostra. Só entram os casos que tinham aprendizado específico na base.
#[derive(Default, Clone)]
struct MemTally {
    cases: usize,
    cases_with_truth: usize,
    truth: usize,
    hits: usize,
    /// Aprendizados específicos entregues por inteiro (a base da precisão).
    delivered: usize,
    /// Certos que não vieram por inteiro mas estavam como linha do índice.
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

/// Soma da previsão de arquivos: os citados contra os vizinhos da co-mudança.
#[derive(Default, Clone)]
struct FileTally {
    cases: usize,
    actual: usize,
    seeds: usize,
    seed_hits: usize,
    suggested: usize,
    suggested_hits: usize,
    /// Casos em que algum vizinho foi sugerido, e a soma da precisão deles (média por caso, sem o peso dos casos enormes).
    cases_suggested: usize,
    suggested_precision_sum: f64,
    /// Soma, por caso, do recall dos citados + vizinhos.
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

/// Cabeçalho do bloco de textos por inteiro, como em `learnings::render_with`.
const FULL_HEADER: &str = "Valem para este trabalho (projeto todo ou os arquivos em questão):\n\n";

/// Entrega em camadas, dentro de `budget`: primeiro os aprendizados dos caminhos citados, depois os dos vizinhos da
/// co-mudança (`extra`), por último os globais. Com `extra` vazio dá o mesmo que `learnings::render_with(.., 0)`; a
/// diferença é que um vizinho nunca tira o lugar de um aprendizado que o próprio pedido já pedia.
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

// ---------- Montagem dos casos ----------

/// Os aprendizados na árvore de `base`, como o `learnings::load` os leria lá. `blobs` guarda o que já foi lido.
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
                learnings::parse(path, &agentdeck_core::util::decode_text(&raw)).ok()
            })
            .clone();
        out.extend(learning);
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// `git ls-tree -r -l`: (caminho, blob, tamanho).
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
        let base_time: i64 =
            repo.text(&["log", "-1", "--format=%ct", &case.base])?.trim().parse().map_err(|_| "data da base".to_owned())?;
        let file_seeds: Vec<String> = seeds.iter().filter(|s| !is_noise(s)).cloned().collect();
        let neigh = HALF_LIVES
            .iter()
            .flat_map(|h| [false, true].map(|lift| neighbours(self.hist, &ancestors, base_time, &file_seeds, &exists, *h, lift)))
            .collect();
        let popular = popular(self.hist, &ancestors, &exists, &file_seeds);
        Ok(Prepared { id: case.id.clone(), sample: case.sample, learnings: all, seeds, truth, actual, neigh, popular })
    }
}

/// Os merges "Merge tarefa tN: …" do histórico: id da tarefa → (merge, pai 1, pai 2).
fn task_merges(repo: &Repo) -> Result<HashMap<String, (String, String, String)>, String> {
    let log = repo.text(&["log", "--merges", "--format=%H\t%P\t%s", "HEAD"])?;
    let mut out = HashMap::new();
    for line in log.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(hash), Some(parents), Some(subject)) = (f.next(), f.next(), f.next()) else { continue };
        let Some(rest) = subject.strip_prefix("Merge tarefa ") else { continue };
        let Some((id, _)) = rest.split_once(':') else { continue };
        let ps: Vec<&str> = parents.split_whitespace().collect();
        if ps.len() == 2 {
            out.entry(id.trim().to_owned()).or_insert((hash.to_owned(), ps[0].to_owned(), ps[1].to_owned()));
        }
    }
    Ok(out)
}

/// Onde a tarefa começou: o pai do commit mais antigo da própria linha da branch (o primeiro pai de cada commit).
fn branch_point(repo: &Repo, p1: &str, p2: &str) -> Option<String> {
    let range = format!("{p1}..{p2}");
    let own = repo.text(&["rev-list", "--first-parent", &range]).ok()?;
    let oldest = own.lines().last()?.to_owned();
    Some(repo.text(&["rev-parse", &format!("{oldest}^")]).ok()?.trim().to_owned())
}

fn task_cases(repo: &Repo, tasks_json: &str) -> Result<Vec<Case>, String> {
    let json: serde_json::Value = serde_json::from_str(tasks_json).map_err(|e| format!("arquivo de tarefas: {e}"))?;
    let merges = task_merges(repo)?;
    let mut out = Vec::new();
    for t in json["tasks"].as_array().ok_or("o arquivo de tarefas não tem a lista \"tasks\"")? {
        if t["status"] != "merged" {
            continue;
        }
        let (Some(id), Some(brief)) = (t["id"].as_str(), t["brief"].as_str()) else { continue };
        let Some((merge, p1, p2)) = merges.get(id) else {
            eprintln!("{id}: sem commit de mesclagem no histórico, fora da amostra");
            continue;
        };
        let Some(base) = branch_point(repo, p1, p2) else {
            eprintln!("{id}: não achei a base, fora da amostra");
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

// ---------- Relatório ----------

fn sample_name(s: Sample) -> &'static str {
    match s {
        Sample::Task => "TAREFAS mescladas",
        Sample::Commit => "COMMITS diretos da dev",
    }
}

fn report(cases: &[Prepared], sample: Sample, detail: bool) {
    let pop: Vec<&Prepared> = cases.iter().filter(|p| p.sample == sample).collect();
    println!("\n==================== {} ====================", sample_name(sample));
    let with_seed = pop.iter().filter(|p| !p.seeds.is_empty()).count();
    let with_learn = pop.iter().filter(|p| p.has_specific()).count();
    let with_truth = pop.iter().filter(|p| !p.truth.is_empty()).count();
    println!("casos: {}", pop.len());
    println!("semente (o pedido cita ao menos um caminho que existe no repositório): {} de {}", with_seed, pop.len());
    println!("com aprendizado específico na base: {with_learn}; com gabarito (algum aprendizado casa com o que mudou): {with_truth}");
    println!("A nota usa só os {with_learn} casos com aprendizado na base; o recall só os {with_truth} com gabarito.");
    let scored: Vec<&Prepared> = pop.iter().copied().filter(|p| p.has_specific()).collect();
    if scored.is_empty() {
        return;
    }

    // Estratégias (a), (a+índice real), (b) e (c)
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
    println!("\n--- Memória: o que cada estratégia entrega (teto de {BUDGET} caracteres de textos por inteiro) ---");
    println!("{:<46} {:>8} {:>9} {:>9} {:>9} {:>9}", "estratégia", "recall", "recall*", "precisão", "caracs", "tokens");
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
    line("(a) hoje, só os textos (sem índice)", &a);
    line("(a') hoje de verdade (índice cortado em 2500)", &a_real);
    line("(b) hoje + índice de todos os outros", &b);
    line("    referência: (a) sem teto de caracteres", &a_free);
    line("    referência: todos os específicos por inteiro", &all_full);
    for (name, t) in [("(a') hoje de verdade", &a_real), ("(b)", &b)] {
        println!(
            "  {name}: {} dos {} certos que (a) não entregou inteiros estavam como linha do índice (o agente teria de pedir com --learn).",
            t.in_index_only,
            t.truth - t.hits
        );
    }
    println!("  recall = certos entregues por inteiro ÷ certos (todos os casos juntos); recall* = média do recall de cada caso;");
    println!("  precisão = certos ÷ específicos entregues por inteiro. Tamanho = média por caso, com os globais e o índice.");

    // O teto de caracteres: quanto custa e quanto ganha subi-lo (estratégia (a), sem índice).
    println!("\n--- Efeito do teto de caracteres em (a) ---");
    println!("{:<12} {:>8} {:>9} {:>9} {:>9}", "teto", "recall", "precisão", "caracs", "tokens");
    for cap in [3_000, BUDGET, 9_000, 12_000, usize::MAX] {
        let mut t = MemTally::default();
        for p in &scored {
            t.add(p, &read_delivery(&p.learnings, &learnings::render_with(&p.learnings, &p.seeds, cap, 0)));
        }
        let name = if cap == usize::MAX { "sem teto".to_owned() } else { cap.to_string() };
        println!("{:<12} {:>8} {:>9} {:>9.0} {:>9.0}", name, pct(t.recall()), pct(t.precision()), t.avg_chars(), t.avg_chars() / 4.0);
    }

    // Grade da co-mudança
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
    // Controle: os arquivos mais mudados, sem olhar o pedido.
    grid.extend(KS.iter().map(|&k| (None, k, 1, 0.0)));
    let label = |key: Option<(usize, bool)>, k: usize, min_count: u32, min_score: f64| {
        let name = key.map_or("ctrl".to_owned(), |(h, lift)| {
            let half = HALF_LIVES[h].map_or("sem".to_owned(), |d| format!("{d}d"));
            format!("{half}/{}", if lift { "lift" } else { "conf" })
        });
        format!("{name:<8} {k:>4} {min_count:>5} {min_score:>5.1}")
    };
    println!("\n--- (c) co-mudança para a memória: grade de limiares ({} casos) ---", scored.len());
    println!("ctrl = controle: os arquivos mais mudados antes da base, sem olhar o pedido.");
    println!("decai = meia-vida em dias; viz = no máximo de vizinhos; vezes = mudaram juntos ao menos; peso = soma das confianças.");
    println!("Camadas: os aprendizados dos citados primeiro, depois os dos vizinhos, dentro do teto. s/teto = o mesmo sem o teto.");
    println!(
        "{:<8} {:>4} {:>5} {:>5} | {:>8} {:>9} {:>8} | {:>10} {:>11} {:>10}",
        "decai", "viz", "vezes", "peso", "recall", "precisão", "tokens", "rec.s/teto", "prec.s/teto", "tok.s/teto"
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

    println!("\n--- Previsão de arquivos pela co-mudança: grade de limiares ({} casos com semente e arquivos mudados) ---", file_pop.len());
    println!("sugeridos = vizinhos por caso; prec.viz = vizinhos que o caso de fato mudou ÷ vizinhos (prec.viz* = média por caso);");
    println!("rec.tot / prec.tot = citados + vizinhos contra os arquivos mudados (rec.tot* = média por caso).");
    println!(
        "{:<8} {:>4} {:>5} {:>5} | {:>9} {:>8} {:>9} | {:>8} {:>9} {:>9}",
        "decai", "viz", "vezes", "peso", "sugeridos", "prec.viz", "prec.viz*", "rec.tot", "rec.tot*", "prec.tot"
    );
    let mut seeds_only = FileTally::default();
    for p in &file_pop {
        seeds_only.add(p, &[]);
    }
    println!(
        "só os citados (sem co-mudança): precisão {} recall {} recall* {} ({} arquivos citados, {} mudados)",
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
        println!("\n--- Por caso ---");
        for p in &pop {
            let d = read_delivery(&p.learnings, &learnings::render_with(&p.learnings, &p.seeds, BUDGET, 0));
            let got: Vec<&String> = p.truth.iter().filter(|t| d.full.contains(*t)).collect();
            println!(
                "{:<8} sementes {:>2} | aprendizados na base {:>2} | certos {:>2} | (a) acertou {:>2} | arquivos mudados {:>3}",
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
        eprintln!("uso: memeval <repositório> <arquivo de tarefas> [--commits-min-arquivos N] [--detalhe]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args[0], &args[1], min_files, detail) {
        eprintln!("erro: {e}");
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
    let tasks_json = agentdeck_core::util::decode_text(&tasks_json);
    let hist = parse_history(
        &repo.text(&["log", "--no-merges", "--name-only", "--format=%x01%H %ct", "HEAD"]).map_err(|e| format!("histórico: {e}"))?,
    );
    eprintln!("histórico: {} commits úteis para a co-mudança", hist.commits.len());
    let mut cases = task_cases(&repo, &tasks_json)?;
    cases.extend(commit_cases(&repo, min_files)?);
    eprintln!("{} casos; preparando…", cases.len());
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
            body: "corpo".into(),
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
        // a mudou em 2 commits: b junto em 1, c junto em 1 → confiança 0,5 cada, desempate pelo caminho.
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
        // aaa (com b) é 7 dias mais velho que bbb (com c); a base vem logo depois de bbb.
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
        let all = vec![learning("memory/aprendizados/a.md", &["src/a.rs"]), learning("memory/aprendizados/b.md", &["src/b.rs"])];
        let rendered = learnings::render_with(&all, &["src/a.rs".to_owned()], BUDGET, usize::MAX);
        let d = read_delivery(&all, &rendered);
        assert_eq!(d.full, ["memory/aprendizados/a.md".to_owned()].into_iter().collect());
        assert_eq!(d.index, ["memory/aprendizados/b.md".to_owned()].into_iter().collect());
        assert_eq!(d.chars, rendered.chars().count());
        let d = read_delivery(&all, &learnings::render_with(&all, &["src/a.rs".to_owned()], BUDGET, 0));
        assert!(d.index.is_empty());
    }

    #[test]
    fn tally_scores_recall_precision_and_index() {
        let all = vec![
            learning("memory/aprendizados/a.md", &["src/a.rs"]),
            learning("memory/aprendizados/b.md", &["src/b.rs"]),
            learning("memory/aprendizados/c.md", &["src/c.rs"]),
        ];
        // O certo é a e b; o palpite entrega a (certo) e c (errado); b só aparece no índice.
        let p = prepared(all.clone(), &["memory/aprendizados/a.md", "memory/aprendizados/b.md"]);
        let rendered = learnings::render_with(&all, &["src/a.rs".to_owned(), "src/c.rs".to_owned()], BUDGET, usize::MAX);
        let mut t = MemTally::default();
        t.add(&p, &read_delivery(&all, &rendered));
        assert_eq!((t.hits, t.truth, t.delivered, t.in_index_only), (1, 2, 2, 1));
        assert!((t.recall() - 0.5).abs() < 1e-9 && (t.precision() - 0.5).abs() < 1e-9 && (t.recall_macro() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tally_ignores_cases_without_truth_in_recall_but_counts_their_delivery() {
        let all = vec![learning("memory/aprendizados/a.md", &["src/a.rs"])];
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
        let mut all = vec![learning("memory/aprendizados/a.md", &["src/a.rs"]), learning("memory/aprendizados/b.md", &["src/b.rs"])];
        all.push(learning("memory/aprendizados/g.md", &[]));
        let seeds = vec!["src/b.rs".to_owned()];
        assert_eq!(layered(&all, &seeds, &[], BUDGET), learnings::render_with(&all, &seeds, BUDGET, 0));
        // O teto só cabe um texto: o do citado (b) vale mais que o do vizinho (a), mesmo que a venha antes na pasta.
        let one = all[1].full_text().chars().count();
        let rendered = layered(&all, &seeds, &["src/a.rs".to_owned()], one);
        assert!(rendered.contains(&all[1].full_text()) && !rendered.contains(&all[0].full_text()));
        // Sem teto, entram os dois e o global, uma vez cada.
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
        assert!(is_noise("memory/GERAL.md") && is_noise("Cargo.lock"));
        assert!(!is_noise("crates/core/Cargo.lock.rs") && !is_noise("src/memory/x.rs"));
    }
}
