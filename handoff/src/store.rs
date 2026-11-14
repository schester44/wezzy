use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Who did something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    You,
    Agent,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::You => "you",
            Actor::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "you" | "me" | "human" | "user" => Ok(Actor::You),
            "agent" => Ok(Actor::Agent),
            _ => bail!("unknown actor {s:?} (you, agent)"),
        }
    }

    /// The lane role whose cards wait on this actor.
    pub fn role(self) -> Role {
        match self {
            Actor::You => Role::You,
            Actor::Agent => Role::Agent,
        }
    }

    pub fn other(self) -> Actor {
        match self {
            Actor::You => Actor::Agent,
            Actor::Agent => Actor::You,
        }
    }
}

/// What a lane means for the cards in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// waiting on you
    You,
    /// waiting on the agent
    Agent,
    /// no owner: in progress, parked, …
    None,
    /// finished
    Done,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::You => "you",
            Role::Agent => "agent",
            Role::None => "none",
            Role::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "you" | "me" | "human" | "user" => Ok(Role::You),
            "agent" => Ok(Role::Agent),
            "none" | "" => Ok(Role::None),
            "done" => Ok(Role::Done),
            _ => bail!("unknown lane role {s:?} (you, agent, none, done)"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Project {
    pub id: i64,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lane {
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    pub name: String,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Card {
    pub id: i64,
    #[serde(skip)]
    pub project_id: i64,
    #[serde(skip)]
    pub lane_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub created_by: Actor,
    /// The agent this card belongs to: the one that filed it or picked it up.
    /// `None`: nobody's yet (a card you added), so any agent's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Where it came from: the repository the agent (or you) was in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// unix seconds
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    pub replies: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reply {
    pub id: i64,
    pub author: Actor,
    pub body: String,
    pub at: i64,
}

/// A card with where it sits and its thread.
#[derive(Clone, Debug, Serialize)]
pub struct CardDetail {
    pub project: String,
    pub lane: String,
    pub role: Role,
    #[serde(flatten)]
    pub card: Card,
    pub thread: Vec<Reply>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LaneCards {
    #[serde(flatten)]
    pub lane: Lane,
    pub cards: Vec<Card>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Board {
    pub project: Project,
    pub lanes: Vec<LaneCards>,
}

/// One entry of the change log. `title` is the card's (or lane's) name when it
/// happened, so the entry still reads after a delete.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<i64>,
    pub actor: Actor,
    /// `card.created`, `card.updated`, `card.moved`, `card.completed`,
    /// `card.reopened`, `card.replied`, `card.deleted`, `lane.created`,
    /// `lane.updated`, `lane.deleted`
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub at: i64,
}

/// What a reader hasn't seen: the other side's changes since its cursor, and
/// the cards waiting on it right now.
#[derive(Clone, Debug, Serialize)]
pub struct Updates {
    pub project: String,
    pub events: Vec<Event>,
    pub queue: Vec<Card>,
}

#[derive(Default, Debug)]
pub struct CardPatch {
    pub title: Option<String>,
    pub body: Option<String>,
}

#[derive(Default, Debug)]
pub struct LanePatch {
    pub name: Option<String>,
    pub role: Option<Role>,
    /// new index among the project's lanes
    pub index: Option<usize>,
}

pub struct Store {
    db: Connection,
    /// Which agent this process is, when it acts as one (`agent_from_env`).
    /// Its cards and the news about them are its own; without one, an agent
    /// sees every agent's.
    pub agent: Option<String>,
    /// The tag new cards get: the repository this process runs in.
    pub tag: Option<String>,
}

const SCHEMA: &str = "
CREATE TABLE projects(
    id INTEGER PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE lanes(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    position INTEGER NOT NULL
);
CREATE TABLE cards(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    lane_id INTEGER NOT NULL REFERENCES lanes(id),
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE INDEX cards_lane ON cards(lane_id, position);
CREATE TABLE replies(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX replies_card ON replies(card_id);
CREATE TABLE events(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    card_id INTEGER,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',
    at INTEGER NOT NULL
);
CREATE INDEX events_project ON events(project_id, id);
CREATE TABLE cursors(
    consumer TEXT NOT NULL,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    event_id INTEGER NOT NULL,
    PRIMARY KEY(consumer, project_id)
);
";

/// The lanes a new project starts with.
const DEFAULT_LANES: [(&str, Role); 4] = [
    ("Needs you", Role::You),
    ("Agent queue", Role::Agent),
    ("In progress", Role::None),
    ("Done", Role::Done),
];

const CARD_COLS: &str = "c.id, c.project_id, c.lane_id, c.title, c.body, c.created_by, \
    c.created_at, c.updated_at, c.completed_at, \
    (SELECT count(*) FROM replies r WHERE r.card_id = c.id), c.agent, c.tag";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `$HANDOFF_DB`, or `~/.local/share/handoff/handoff.db`.
pub fn default_db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("HANDOFF_DB").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".local/share/handoff/handoff.db")
}

/// Which agent this process is: `$HANDOFF_AGENT`, else the terminal pane it
/// runs in (`pane:<id>`, from WezTerm / Wezzy or tmux). An agent's MCP server
/// and hooks inherit both, so they agree, and a session resumed in the same
/// pane is the same agent.
pub fn agent_from_env() -> Option<String> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    var("HANDOFF_AGENT")
        .or_else(|| var("WEZTERM_PANE").map(|p| format!("pane:{p}")))
        .or_else(|| var("TMUX_PANE").map(|p| format!("pane:{p}")))
}

/// Lowercase, with runs of anything that isn't a letter or digit as one `-`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The project a directory belongs to: the name of its git repository (the
/// main one, from inside a worktree), or of the directory itself.
pub fn project_for_dir(dir: &Path) -> String {
    let mut at = Some(dir);
    while let Some(d) = at {
        let git = d.join(".git");
        if git.is_dir() {
            return dir_slug(d);
        }
        if git.is_file() {
            // a worktree: `gitdir: /repo/.git/worktrees/<name>`
            let main = std::fs::read_to_string(&git).ok().and_then(|s| {
                let path = s.trim().strip_prefix("gitdir:")?.trim().to_string();
                let at = path.find("/.git/worktrees/")?;
                Some(PathBuf::from(&path[..at]))
            });
            return dir_slug(main.as_deref().unwrap_or(d));
        }
        at = d.parent();
    }
    dir_slug(dir)
}

fn dir_slug(dir: &Path) -> String {
    slugify(&dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())
}

fn card_row(r: &Row) -> rusqlite::Result<Card> {
    let by: String = r.get(5)?;
    Ok(Card {
        id: r.get(0)?,
        project_id: r.get(1)?,
        lane_id: r.get(2)?,
        title: r.get(3)?,
        body: r.get(4)?,
        created_by: Actor::parse(&by).unwrap_or(Actor::You),
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        completed_at: r.get(8)?,
        replies: r.get(9)?,
        agent: r.get(10)?,
        tag: r.get(11)?,
    })
}

fn lane_row(r: &Row) -> rusqlite::Result<Lane> {
    let role: String = r.get(3)?;
    Ok(Lane {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        role: Role::parse(&role).unwrap_or(Role::None),
    })
}

fn project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project { id: r.get(0)?, slug: r.get(1)?, name: r.get(2)? })
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(db)
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&default_db_path())
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            // two processes can both see 0; the second finds the tables there
            let tx = db.unchecked_transaction()?;
            let made: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'projects'",
                [],
                |r| r.get(0),
            )?;
            if made == 0 {
                tx.execute_batch(SCHEMA)?;
            }
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 1 {
            // v2: cards belong to an agent, and an event is delivered once to
            // the side that didn't make it (instead of a cursor per reader)
            let tx = db.unchecked_transaction()?;
            let has: i64 = tx.query_row(
                "SELECT count(*) FROM pragma_table_info('cards') WHERE name = 'agent'",
                [],
                |r| r.get(0),
            )?;
            if has == 0 {
                tx.execute_batch(
                    "ALTER TABLE cards ADD COLUMN agent TEXT;
                     ALTER TABLE events ADD COLUMN agent TEXT;
                     ALTER TABLE events ADD COLUMN delivered INTEGER NOT NULL DEFAULT 0;
                     UPDATE events SET delivered = 1;
                     DROP TABLE cursors;
                     CREATE INDEX events_pending ON events(delivered, project_id);",
                )?;
            }
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 2 {
            // v3: a card is tagged with where it came from
            let tx = db.unchecked_transaction()?;
            let has: i64 = tx.query_row(
                "SELECT count(*) FROM pragma_table_info('cards') WHERE name = 'tag'",
                [],
                |r| r.get(0),
            )?;
            if has == 0 {
                tx.execute_batch("ALTER TABLE cards ADD COLUMN tag TEXT;")?;
            }
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        Ok(Store { db, agent: None, tag: None })
    }

    /// Changes whenever another connection writes: cheap to poll.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.db.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    // ---- projects

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut q = self.db.prepare("SELECT id, slug, name FROM projects ORDER BY name COLLATE NOCASE")?;
        let rows = q.query_map([], project_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn find_project(&self, slug: &str) -> Result<Option<Project>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, slug, name FROM projects WHERE slug = ?1",
                [slugify(slug)],
                project_row,
            )
            .optional()?)
    }

    pub fn project(&self, slug: &str) -> Result<Project> {
        self.find_project(slug)?.ok_or_else(|| anyhow!("no project {slug:?}"))
    }

    fn project_by_id(&self, id: i64) -> Result<Project> {
        Ok(self.db.query_row("SELECT id, slug, name FROM projects WHERE id = ?1", [id], project_row)?)
    }

    /// The project named `name`, made with the default lanes if it's new.
    pub fn ensure_project(&self, name: &str) -> Result<Project> {
        let slug = slugify(name);
        if slug.is_empty() {
            bail!("a project needs a name");
        }
        if let Some(p) = self.find_project(&slug)? {
            return Ok(p);
        }
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO projects(slug, name, created_at) VALUES (?1, ?2, ?3)",
            params![slug, name.trim(), now()],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM projects WHERE slug = ?1", [&slug], |r| r.get(0))?;
        let lanes: i64 =
            tx.query_row("SELECT count(*) FROM lanes WHERE project_id = ?1", [id], |r| r.get(0))?;
        if lanes == 0 {
            for (i, (lane, role)) in DEFAULT_LANES.iter().enumerate() {
                tx.execute(
                    "INSERT INTO lanes(project_id, name, role, position) VALUES (?1, ?2, ?3, ?4)",
                    params![id, lane, role.as_str(), i as i64],
                )?;
            }
        }
        tx.commit()?;
        self.project_by_id(id)
    }

    pub fn rename_project(&self, slug: &str, name: &str) -> Result<Project> {
        let p = self.project(slug)?;
        self.db.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![name.trim(), p.id])?;
        self.project_by_id(p.id)
    }

    /// Deletes the project with its lanes, cards and history.
    pub fn delete_project(&self, slug: &str) -> Result<()> {
        let p = self.project(slug)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM cards WHERE project_id = ?1", [p.id])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [p.id])?;
        tx.commit()?;
        Ok(())
    }

    // ---- lanes

    pub fn lanes(&self, project_id: i64) -> Result<Vec<Lane>> {
        let mut q = self.db.prepare(
            "SELECT id, project_id, name, role FROM lanes WHERE project_id = ?1 ORDER BY position, id",
        )?;
        let rows = q.query_map([project_id], lane_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn lane(&self, id: i64) -> Result<Lane> {
        self.db
            .query_row("SELECT id, project_id, name, role FROM lanes WHERE id = ?1", [id], lane_row)
            .optional()?
            .ok_or_else(|| anyhow!("no lane {id}"))
    }

    /// A lane by name (any case), role (`you`, `agent`, `done`: the first with
    /// it) or id.
    pub fn find_lane(&self, project_id: i64, key: &str) -> Result<Lane> {
        let lanes = self.lanes(project_id)?;
        let key = key.trim();
        let found = lanes
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(key))
            .or_else(|| lanes.iter().find(|l| slugify(&l.name) == slugify(key)))
            .or_else(|| {
                let role = Role::parse(key).ok().filter(|r| *r != Role::None)?;
                lanes.iter().find(|l| l.role == role)
            })
            .or_else(|| {
                let id: i64 = key.parse().ok()?;
                lanes.iter().find(|l| l.id == id)
            });
        found.cloned().ok_or_else(|| {
            let names: Vec<_> = lanes.iter().map(|l| l.name.as_str()).collect();
            anyhow!("no lane {key:?} (lanes: {})", names.join(", "))
        })
    }

    fn first_lane(&self, project_id: i64, role: Role) -> Result<Lane> {
        let lanes = self.lanes(project_id)?;
        lanes
            .iter()
            .find(|l| l.role == role)
            .cloned()
            .ok_or_else(|| anyhow!("this project has no lane with the role `{}`", role.as_str()))
    }

    pub fn add_lane(&self, project_id: i64, name: &str, role: Role, actor: Actor) -> Result<Lane> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a lane needs a name");
        }
        let tx = self.db.unchecked_transaction()?;
        let next: i64 = tx.query_row(
            "SELECT coalesce(max(position), -1) + 1 FROM lanes WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO lanes(project_id, name, role, position) VALUES (?1, ?2, ?3, ?4)",
            params![project_id, name, role.as_str(), next],
        )?;
        let id = tx.last_insert_rowid();
        self.event(&tx, project_id, None, actor, "lane.created", name, "")?;
        tx.commit()?;
        self.lane(id)
    }

    pub fn update_lane(&self, id: i64, patch: LanePatch, actor: Actor) -> Result<Lane> {
        let lane = self.lane(id)?;
        let tx = self.db.unchecked_transaction()?;
        if let Some(name) = patch.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            tx.execute("UPDATE lanes SET name = ?1 WHERE id = ?2", params![name, id])?;
        }
        if let Some(role) = patch.role {
            tx.execute("UPDATE lanes SET role = ?1 WHERE id = ?2", params![role.as_str(), id])?;
            // the lane's cards are finished exactly when the lane says so
            if role == Role::Done {
                tx.execute(
                    "UPDATE cards SET completed_at = ?1 WHERE lane_id = ?2 AND completed_at IS NULL",
                    params![now(), id],
                )?;
            } else {
                tx.execute("UPDATE cards SET completed_at = NULL WHERE lane_id = ?1", [id])?;
            }
        }
        if let Some(index) = patch.index {
            let mut ids: Vec<i64> = self.lanes(lane.project_id)?.iter().map(|l| l.id).collect();
            ids.retain(|l| *l != id);
            ids.insert(index.min(ids.len()), id);
            for (i, lane_id) in ids.iter().enumerate() {
                tx.execute("UPDATE lanes SET position = ?1 WHERE id = ?2", params![i as i64, lane_id])?;
            }
        }
        let name = patch.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&lane.name);
        self.event(&tx, lane.project_id, None, actor, "lane.updated", name, "")?;
        tx.commit()?;
        self.lane(id)
    }

    /// Deletes an empty lane. Cards have to be moved or deleted first.
    pub fn delete_lane(&self, id: i64, actor: Actor) -> Result<()> {
        let lane = self.lane(id)?;
        let cards: i64 =
            self.db.query_row("SELECT count(*) FROM cards WHERE lane_id = ?1", [id], |r| r.get(0))?;
        if cards > 0 {
            bail!("{:?} still has {cards} card(s): move or delete them first", lane.name);
        }
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM lanes WHERE id = ?1", [id])?;
        self.event(&tx, lane.project_id, None, actor, "lane.deleted", &lane.name, "")?;
        tx.commit()?;
        Ok(())
    }

    // ---- cards

    fn lane_cards(&self, lane_id: i64) -> Result<Vec<Card>> {
        let mut q = self.db.prepare(&format!(
            "SELECT {CARD_COLS} FROM cards c WHERE c.lane_id = ?1 ORDER BY c.position, c.id"
        ))?;
        let rows = q.query_map([lane_id], card_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn board(&self, project: &Project) -> Result<Board> {
        let mut lanes = Vec::new();
        for lane in self.lanes(project.id)? {
            let cards = self.lane_cards(lane.id)?;
            lanes.push(LaneCards { lane, cards });
        }
        Ok(Board { project: project.clone(), lanes })
    }

    fn card_only(&self, id: i64) -> Result<Card> {
        self.db
            .query_row(&format!("SELECT {CARD_COLS} FROM cards c WHERE c.id = ?1"), [id], card_row)
            .optional()?
            .ok_or_else(|| anyhow!("no card #{id}"))
    }

    pub fn card(&self, id: i64) -> Result<CardDetail> {
        let card = self.card_only(id)?;
        let lane = self.lane(card.lane_id)?;
        let project = self.project_by_id(card.project_id)?;
        let mut q = self.db.prepare(
            "SELECT id, author, body, created_at FROM replies WHERE card_id = ?1 ORDER BY id",
        )?;
        let thread = q
            .query_map([id], |r| {
                let author: String = r.get(1)?;
                Ok(Reply {
                    id: r.get(0)?,
                    author: Actor::parse(&author).unwrap_or(Actor::You),
                    body: r.get(2)?,
                    at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(CardDetail { project: project.slug, lane: lane.name, role: lane.role, card, thread })
    }

    /// A new card at the bottom of `lane`, or of the first lane that hands it
    /// to the other side: an agent's card lands in front of you, yours in the
    /// agent's queue.
    pub fn add_card(
        &self,
        project_id: i64,
        lane: Option<i64>,
        title: &str,
        body: &str,
        actor: Actor,
    ) -> Result<Card> {
        let title = title.trim();
        if title.is_empty() {
            bail!("a card needs a title");
        }
        let lane = match lane {
            Some(id) => {
                let lane = self.lane(id)?;
                if lane.project_id != project_id {
                    bail!("lane {id} belongs to another project");
                }
                lane
            }
            None => match self.first_lane(project_id, actor.other().role()) {
                Ok(lane) => lane,
                Err(_) => self
                    .lanes(project_id)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("this project has no lanes"))?,
            },
        };
        let at = now();
        let tx = self.db.unchecked_transaction()?;
        let next: i64 = tx.query_row(
            "SELECT coalesce(max(position), -1) + 1 FROM cards WHERE lane_id = ?1",
            [lane.id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO cards(project_id, lane_id, title, body, position, created_by, created_at, \
             updated_at, completed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
            params![
                project_id,
                lane.id,
                title,
                body.trim(),
                next,
                actor.as_str(),
                at,
                (lane.role == Role::Done).then_some(at)
            ],
        )?;
        let id = tx.last_insert_rowid();
        if let Some(tag) = self.tag.as_deref().filter(|t| !t.is_empty()) {
            tx.execute("UPDATE cards SET tag = ?1 WHERE id = ?2", params![tag, id])?;
        }
        self.event(&tx, project_id, Some(id), actor, "card.created", title, &lane.name)?;
        tx.commit()?;
        self.card_only(id)
    }

    pub fn update_card(&self, id: i64, patch: CardPatch, actor: Actor) -> Result<Card> {
        let card = self.card_only(id)?;
        let title = patch.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if title.is_none() && patch.body.is_none() {
            return Ok(card);
        }
        let tx = self.db.unchecked_transaction()?;
        if let Some(title) = title {
            tx.execute("UPDATE cards SET title = ?1 WHERE id = ?2", params![title, id])?;
        }
        if let Some(body) = &patch.body {
            tx.execute("UPDATE cards SET body = ?1 WHERE id = ?2", params![body.trim(), id])?;
        }
        tx.execute("UPDATE cards SET updated_at = ?1 WHERE id = ?2", params![now(), id])?;
        self.event(&tx, card.project_id, Some(id), actor, "card.updated", title.unwrap_or(&card.title), "")?;
        tx.commit()?;
        self.card_only(id)
    }

    /// Moves a card to `index` in `lane_id` (the bottom when `None`). Landing
    /// in a `done` lane completes it; leaving one reopens it.
    pub fn move_card(&self, id: i64, lane_id: i64, index: Option<usize>, actor: Actor) -> Result<Card> {
        let card = self.card_only(id)?;
        let to = self.lane(lane_id)?;
        if to.project_id != card.project_id {
            bail!("lane {lane_id} belongs to another project");
        }
        let from = self.lane(card.lane_id)?;
        let mut order: Vec<i64> = self.lane_cards(lane_id)?.iter().map(|c| c.id).collect();
        order.retain(|c| *c != id);
        let index = index.unwrap_or(order.len()).min(order.len());
        order.insert(index, id);

        let at = now();
        let tx = self.db.unchecked_transaction()?;
        for (i, card_id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE cards SET lane_id = ?1, position = ?2 WHERE id = ?3",
                params![lane_id, i as i64, card_id],
            )?;
        }
        if from.id != to.id {
            let done = to.role == Role::Done;
            tx.execute(
                "UPDATE cards SET updated_at = ?1, completed_at = ?2 WHERE id = ?3",
                params![at, done.then_some(card.completed_at.unwrap_or(at)), id],
            )?;
            let kind = match (from.role == Role::Done, done) {
                (false, true) => "card.completed",
                (true, false) => "card.reopened",
                _ => "card.moved",
            };
            let detail = format!("{} → {}", from.name, to.name);
            self.event(&tx, card.project_id, Some(id), actor, kind, &card.title, &detail)?;
        }
        tx.commit()?;
        self.card_only(id)
    }

    /// Moves a card to the top of the first `done` lane, with an optional
    /// closing note in its thread.
    pub fn complete_card(&self, id: i64, note: Option<&str>, actor: Actor) -> Result<Card> {
        let card = self.card_only(id)?;
        if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
            self.reply(id, note, actor)?;
        }
        if self.lane(card.lane_id)?.role == Role::Done {
            return self.card_only(id);
        }
        let done = self.first_lane(card.project_id, Role::Done)?;
        self.move_card(id, done.id, Some(0), actor)
    }

    pub fn reply(&self, id: i64, body: &str, actor: Actor) -> Result<Reply> {
        let card = self.card_only(id)?;
        let body = body.trim();
        if body.is_empty() {
            bail!("a reply needs some text");
        }
        let at = now();
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO replies(card_id, author, body, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, actor.as_str(), body, at],
        )?;
        let reply = Reply { id: tx.last_insert_rowid(), author: actor, body: body.to_string(), at };
        tx.execute("UPDATE cards SET updated_at = ?1 WHERE id = ?2", params![at, id])?;
        self.event(&tx, card.project_id, Some(id), actor, "card.replied", &card.title, body)?;
        tx.commit()?;
        Ok(reply)
    }

    pub fn delete_card(&self, id: i64, actor: Actor) -> Result<()> {
        let card = self.card_only(id)?;
        let tx = self.db.unchecked_transaction()?;
        // logged first: the event takes the card's owner
        self.event(&tx, card.project_id, Some(id), actor, "card.deleted", &card.title, "")?;
        tx.execute("DELETE FROM cards WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    // ---- closing the gap

    /// The cards waiting on `actor` in a project, lane by lane. For an agent
    /// with an identity: only its own.
    pub fn queue(&self, project_id: i64, actor: Actor) -> Result<Vec<Card>> {
        self.queue_as(project_id, actor, self.agent.as_deref())
    }

    fn queue_as(&self, project_id: i64, actor: Actor, agent: Option<&str>) -> Result<Vec<Card>> {
        let mine = match (actor, agent) {
            (Actor::Agent, Some(_)) => "AND c.agent = ?3",
            _ => "AND ?3 IS NULL",
        };
        let agent = if actor == Actor::Agent { agent } else { None };
        let mut q = self.db.prepare(&format!(
            "SELECT {CARD_COLS} FROM cards c JOIN lanes l ON l.id = c.lane_id \
             WHERE c.project_id = ?1 AND l.role = ?2 {mine} ORDER BY l.position, c.position, c.id"
        ))?;
        let rows = q
            .query_map(params![project_id, actor.role().as_str(), agent], card_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// How many cards wait on `actor`, per project that has any.
    pub fn waiting(&self, actor: Actor) -> Result<Vec<(Project, i64)>> {
        let mut q = self.db.prepare(
            "SELECT p.id, p.slug, p.name, count(*) FROM cards c \
             JOIN lanes l ON l.id = c.lane_id JOIN projects p ON p.id = c.project_id \
             WHERE l.role = ?1 GROUP BY p.id ORDER BY p.name COLLATE NOCASE",
        )?;
        let rows = q
            .query_map([actor.role().as_str()], |r| Ok((project_row(r)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The cards waiting on `actor` on every board, counted by where they came
    /// from: a card's tag (its repository), or its board when it has none.
    /// Most first.
    pub fn waiting_by_source(&self, actor: Actor) -> Result<Vec<(String, i64)>> {
        let mut q = self.db.prepare(
            "SELECT coalesce(nullif(c.tag, ''), p.name) AS source, count(*) AS n FROM cards c \
             JOIN lanes l ON l.id = c.lane_id JOIN projects p ON p.id = c.project_id \
             WHERE l.role = ?1 GROUP BY source ORDER BY n DESC, source COLLATE NOCASE",
        )?;
        let rows = q
            .query_map([actor.role().as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// What `reader` hasn't been told yet in a project (the other side's
    /// changes), plus the cards waiting on it now. Each change is delivered
    /// once, unless `peek`. An agent with an identity gets the news about its
    /// own cards only: with many agents on a project, nobody hears about work
    /// that isn't theirs. A card nobody owns waits to be assigned or taken.
    pub fn updates(&self, project: &Project, reader: Actor, peek: bool) -> Result<Updates> {
        self.updates_as(project, reader, self.agent.as_deref(), peek)
    }

    /// `updates` on behalf of `agent` (a frontend delivering for it).
    pub fn updates_as(&self, project: &Project, reader: Actor, agent: Option<&str>, peek: bool) -> Result<Updates> {
        let agent = if reader == Actor::Agent { agent } else { None };
        let tx = self.db.unchecked_transaction()?;
        let events: Vec<Event> = {
            let mut q = tx.prepare(
                "SELECT id, card_id, actor, kind, title, detail, at FROM events \
                 WHERE project_id = ?1 AND delivered = 0 AND actor != ?2 \
                 AND (?3 IS NULL OR agent = ?3) ORDER BY id",
            )?;
            let rows = q
                .query_map(params![project.id, reader.as_str(), agent], |r| {
                    let actor: String = r.get(2)?;
                    Ok(Event {
                        id: r.get(0)?,
                        card: r.get(1)?,
                        actor: Actor::parse(&actor).unwrap_or(Actor::You),
                        kind: r.get(3)?,
                        title: r.get(4)?,
                        detail: r.get(5)?,
                        at: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        if !peek {
            for e in &events {
                tx.execute("UPDATE events SET delivered = 1 WHERE id = ?1", [e.id])?;
            }
        }
        tx.commit()?;
        let queue = self.queue_as(project.id, reader, agent)?;
        Ok(Updates { project: project.slug.clone(), events, queue })
    }

    /// Who has news from you waiting: per project, the agents (`None`: any
    /// agent) with undelivered changes of yours.
    pub fn pending(&self) -> Result<Vec<(Project, Option<String>)>> {
        let mut q = self.db.prepare(
            "SELECT DISTINCT p.id, p.slug, p.name, e.agent FROM events e \
             JOIN projects p ON p.id = e.project_id WHERE e.delivered = 0 AND e.actor = 'you'",
        )?;
        let rows = q
            .query_map([], |r| Ok((project_row(r)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn tag_card(&self, id: i64, tag: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE cards SET tag = ?1 WHERE id = ?2", params![tag, id])?;
        Ok(())
    }

    /// Give a card to an agent (or to nobody). What you did on it that hasn't
    /// been delivered goes to the new owner, who is also told it's theirs.
    pub fn assign(&self, id: i64, agent: Option<&str>, actor: Actor) -> Result<Card> {
        let card = self.card_only(id)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute("UPDATE cards SET agent = ?1 WHERE id = ?2", params![agent, id])?;
        // changing your mind before anyone heard leaves one notice, not a trail
        tx.execute(
            "DELETE FROM events WHERE card_id = ?1 AND delivered = 0 AND kind = 'card.assigned'",
            [id],
        )?;
        tx.execute(
            "UPDATE events SET agent = ?1 WHERE card_id = ?2 AND delivered = 0 AND actor = 'you'",
            params![agent, id],
        )?;
        if agent.is_some() {
            self.event(&tx, card.project_id, Some(id), actor, "card.assigned", &card.title, "")?;
        }
        tx.commit()?;
        self.card_only(id)
    }

    /// An agent is gone: its cards belong to nobody until they're assigned
    /// again or another agent takes them.
    pub fn release(&self, agent: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("UPDATE cards SET agent = NULL WHERE agent = ?1", [agent])?;
        tx.execute("UPDATE events SET agent = NULL WHERE agent = ?1 AND delivered = 0", [agent])?;
        tx.commit()?;
        Ok(())
    }

    /// Log a change. An agent acting on a card nobody owns takes it; the
    /// event carries the card's owner, which is who hears about it.
    #[allow(clippy::too_many_arguments)]
    fn event(
        &self,
        db: &Connection,
        project_id: i64,
        card: Option<i64>,
        actor: Actor,
        kind: &str,
        title: &str,
        detail: &str,
    ) -> Result<()> {
        let mut owner: Option<String> = None;
        if let Some(id) = card {
            if let (Actor::Agent, Some(me)) = (actor, self.agent.as_deref()) {
                db.execute("UPDATE cards SET agent = ?1 WHERE id = ?2 AND agent IS NULL", params![me, id])?;
            }
            owner = db
                .query_row("SELECT agent FROM cards WHERE id = ?1", [id], |r| r.get(0))
                .optional()?
                .flatten();
        }
        db.execute(
            "INSERT INTO events(project_id, card_id, actor, kind, title, detail, agent, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project_id, card, actor.as_str(), kind, title, detail, owner, now()],
        )?;
        Ok(())
    }
}
