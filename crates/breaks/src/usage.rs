// Screen time: seconds each program was in front, by day. What is kept is a program's name and a
// number of seconds, never a window title, in one file of the feature's own data folder.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The file, in the feature's data folder.
pub const FILE: &str = "screen-time.json";
/// Days kept, today included: what the history shows.
const KEEP: usize = 7;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct App {
    pub name: String,
    pub secs: u64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Day {
    /// By the program's file name, lower case, without ".exe".
    #[serde(default)]
    pub apps: BTreeMap<String, App>,
    #[serde(default)]
    pub switches: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    /// By local date, "2026-10-11".
    #[serde(default)]
    days: BTreeMap<String, Day>,
}

pub struct Usage {
    /// `None` keeps it in memory only (the probe).
    path: Option<PathBuf>,
    store: Store,
    /// The program in front, as (key, name); `None` while it is nobody's (this app, the lock screen).
    front: Option<(String, String)>,
    /// The last program that was in front, to count a switch only when it changes.
    last: Option<String>,
    dirty: bool,
}

/// Today's local date, the key of a day.
pub fn today() -> String {
    let t = notch_core::win::local_time();
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

impl Usage {
    pub fn load(path: Option<PathBuf>) -> Self {
        let store = path.as_deref().map(notch_core::storage::json_load).unwrap_or_default();
        Usage { path, store, front: None, last: None, dirty: false }
    }

    fn day(&mut self, day: &str) -> &mut Day {
        if !self.store.days.contains_key(day) {
            self.store.days.insert(day.to_string(), Day::default());
            while self.store.days.len() > KEEP {
                self.store.days.pop_first();
            }
        }
        self.store.days.get_mut(day).expect("just inserted")
    }

    /// Another window came to the front.
    pub fn front(&mut self, day: &str, app: Option<(String, String)>) {
        if let Some((key, _)) = &app {
            if self.last.as_ref().is_some_and(|last| last != key) {
                self.day(day).switches += 1;
                self.dirty = true;
            }
            self.last = Some(key.clone());
        }
        self.front = app;
    }

    /// `secs` seconds at the PC go to the program in front.
    pub fn add(&mut self, day: &str, secs: u64) {
        let Some((key, name)) = self.front.clone() else { return };
        let app = self.day(day).apps.entry(key).or_default();
        app.name = name;
        app.secs += secs;
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.store.days.clear();
        self.dirty = true;
        self.save();
    }

    pub fn save(&mut self) {
        if let (true, Some(path)) = (self.dirty, self.path.as_deref()) {
            if let Err(e) = notch_core::storage::json_save_atomic(path, &self.store) {
                notch_core::log::line(format!("breaks: screen time not saved: {e}"));
                return;
            }
        }
        self.dirty = false;
    }

    /// For the page: today's programs by time, the switches, and the total of every day kept.
    pub fn snapshot(&self, day: &str) -> Value {
        let today = self.store.days.get(day).cloned().unwrap_or_default();
        let mut apps: Vec<(&String, &App)> = today.apps.iter().collect();
        apps.sort_by(|a, b| b.1.secs.cmp(&a.1.secs));
        let total = |d: &Day| d.apps.values().map(|a| a.secs).sum::<u64>();
        json!({
            "on": true,
            "today": day,
            "total": total(&today),
            "switches": today.switches,
            "apps": apps.iter().map(|(key, app)| json!({ "name": app.name, "secs": app.secs, "cat": category(key) })).collect::<Vec<_>>(),
            "days": self.store.days.iter().map(|(date, d)| (date.clone(), json!(total(d)))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

// ── Categories ────────────────────────────────────────────────────────────────

/// Programs by what they are for, by key with a trailing version cut off ("idea64" is "idea").
/// debt: a fixed table; the person's own categories and an ignore list come later.
const CATEGORIES: &[(&str, &[&str])] = &[
    (
        "development",
        &[
            "code", "cursor", "windsurf", "zed", "devenv", "idea", "pycharm", "webstorm", "rider", "clion", "goland",
            "rustrover", "phpstorm", "datagrip", "studio", "sublime_text", "notepad++", "windowsterminal", "wt", "cmd",
            "powershell", "pwsh", "conhost", "mintty", "wezterm-gui", "alacritty", "githubdesktop", "gitkraken", "fork",
            "docker desktop", "postman", "insomnia", "dbeaver", "ssms", "wsl", "claude",
        ],
    ),
    (
        "communication",
        &[
            "teams", "ms-teams", "slack", "discord", "zoom", "outlook", "olk", "hxoutlook", "thunderbird", "telegram",
            "whatsapp", "whatsapp.root", "signal", "skype", "webex", "webexhost", "element", "mattermost",
        ],
    ),
    ("browsing", &["chrome", "msedge", "firefox", "brave", "opera", "vivaldi", "arc", "zen", "iexplore", "librewolf", "chromium"]),
    (
        "entertainment",
        &[
            "spotify", "vlc", "steam", "steamwebhelper", "epicgameslauncher", "netflix", "mpc-hc", "mpv", "wmplayer",
            "music.ui", "video.ui", "microsoft.media.player", "itunes", "applemusic", "tidal", "deezer", "plex",
            "gamebar", "xboxapp", "battle.net", "riotclientux", "leagueclientux",
        ],
    ),
    (
        "productivity",
        &[
            "winword", "excel", "powerpnt", "onenote", "onenoteim", "msaccess", "mspub", "visio", "notion", "obsidian",
            "acrobat", "acrord", "sumatrapdf", "notepad", "wordpad", "evernote", "todoist", "soffice", "soffice.bin",
            "logseq", "anki", "calibre", "ticktick",
        ],
    ),
    (
        "design",
        &[
            "figma", "photoshop", "illustrator", "indesign", "afterfx", "adobe premiere pro", "lightroom", "blender",
            "gimp", "inkscape", "mspaint", "krita", "canva", "resolve", "xd", "affinity photo", "affinity designer",
            "paintdotnet", "obs", "sketchup",
        ],
    ),
    (
        "utilities",
        &[
            "explorer", "taskmgr", "systemsettings", "searchhost", "startmenuexperiencehost", "shellexperiencehost",
            "calculatorapp", "calc", "snippingtool", "screenclippinghost", "7zfm", "winrar", "regedit", "mmc", "control",
            "everything", "powertoys", "powertoys.settings", "msiexec", "perfmon", "resmon", "procexp",
        ],
    ),
];

pub fn category(key: &str) -> &'static str {
    let base = key.trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '_' | ' '));
    CATEGORIES.iter().find(|(_, names)| names.contains(&key) || names.contains(&base)).map_or("other", |c| c.0)
}
