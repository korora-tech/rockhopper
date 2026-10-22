//! Installed applications, read from XDG desktop entries.
//!
//! Implements the parts of the Desktop Entry Specification a launcher needs:
//! the `[Desktop Entry]` group, localized keys, visibility rules, `TryExec`
//! and `Exec` quoting with field codes.
//! <https://specifications.freedesktop.org/desktop-entry-spec/latest/>

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    /// Desktop file ID, such as `org.mozilla.firefox.desktop`.
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    /// The `Exec` program and arguments, field codes already expanded.
    pub argv: Vec<String>,
    pub terminal: bool,
    /// Working directory from `Path=`.
    pub cwd: Option<PathBuf>,
    pub icon: Option<String>,
    /// Resolved icon file, filled in by [`crate::icons`].
    pub icon_path: Option<PathBuf>,
    /// `StartupWMClass`, which windows of this app may use as their app ID.
    pub wm_class: Option<String>,
    /// Desktop actions, such as "New Private Window", in `Actions=` order.
    pub actions: Vec<DesktopAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopAction {
    pub name: String,
    pub argv: Vec<String>,
}

impl App {
    /// Whether a window with Wayland app ID `app_id` belongs to this app.
    pub fn owns_window(&self, app_id: &str) -> bool {
        let stem = self.id.strip_suffix(".desktop").unwrap_or(&self.id);
        stem.eq_ignore_ascii_case(app_id)
            || self.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(app_id))
            || self.argv.first().is_some_and(|p| p.rsplit('/').next() == Some(app_id))
    }
}

/// The `applications` directories in XDG precedence order: user data first.
pub fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());

    let mut seen = HashSet::new();
    data_home
        .into_iter()
        .chain(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

/// Cheap change detection: each directory's resolved path and mtime. Nix
/// profile switches change the resolved path; installs change the mtime.
pub fn fingerprint(dirs: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)> {
    dirs.iter()
        .map(|d| {
            let real = std::fs::canonicalize(d).unwrap_or_else(|_| d.clone());
            let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
            (real, mtime)
        })
        .collect()
}

pub struct Env {
    /// `XDG_CURRENT_DESKTOP`, split on `:`.
    pub desktops: Vec<String>,
    /// Locale keys to try, most specific first, such as `["en_GB", "en"]`.
    pub locales: Vec<String>,
    pub path: Vec<PathBuf>,
}

impl Env {
    pub fn current() -> Self {
        let desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .collect();
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default();
        let path = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        Self {
            desktops,
            locales: locale_keys(&locale),
            path,
        }
    }

    fn on_path(&self, program: &str) -> bool {
        if program.contains('/') {
            return is_executable(Path::new(program));
        }
        self.path.iter().any(|dir| is_executable(&dir.join(program)))
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `en_GB.UTF-8@euro` → `["en_GB@euro", "en_GB", "en@euro", "en"]`, the
/// matching order the spec gives for `LC_MESSAGES`.
fn locale_keys(locale: &str) -> Vec<String> {
    if locale.is_empty() || locale == "C" || locale == "POSIX" {
        return Vec::new();
    }
    let (rest, modifier) = match locale.split_once('@') {
        Some((r, m)) => (r, Some(m)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or(rest);
    let (lang, country) = match rest.split_once('_') {
        Some((l, c)) => (l, Some(c)),
        None => (rest, None),
    };
    let mut keys = Vec::new();
    if let (Some(c), Some(m)) = (country, modifier) {
        keys.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        keys.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        keys.push(format!("{lang}@{m}"));
    }
    keys.push(lang.to_owned());
    keys
}

/// Every visible application, first entry per desktop file ID winning, sorted
/// by name.
pub fn scan(dirs: &[PathBuf], env: &Env) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_desktop_files(dir, dir, &mut HashSet::new(), &mut files);
        for (id, path) in files {
            // A hidden or invalid entry still shadows the same ID in later dirs.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = parse(&id, &path, &text, env) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
    apps
}

/// Desktop file IDs are paths below `applications/` with `/` replaced by `-`.
/// Symlinked directories are followed once, so a link loop ends.
fn collect_desktop_files(
    root: &Path,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<(String, PathBuf)>,
) {
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(root, &path, visited, out);
        } else if path.extension().is_some_and(|e| e == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let id = rel.to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}

pub fn parse(id: &str, path: &Path, text: &str, env: &Env) -> Option<App> {
    let groups = groups(text);
    let group = groups.get("Desktop Entry")?;
    let get = |key: &str| group.get(key).map(|v| unescape(v));
    let get_bool = |key: &str| group.get(key).is_some_and(|v| v.trim() == "true");
    let localized_in = |group: &HashMap<String, String>, key: &str| {
        env.locales
            .iter()
            .find_map(|l| group.get(&format!("{key}[{l}]")))
            .or_else(|| group.get(key))
            .map(|v| unescape(v))
    };
    let localized = |key: &str| localized_in(group, key);

    if get("Type").as_deref() != Some("Application") || get_bool("NoDisplay") || get_bool("Hidden")
    {
        return None;
    }
    let listed = |key: &str| get(key).map(|v| split_list(&v));
    if let Some(only) = listed("OnlyShowIn")
        && !only.iter().any(|d| env.desktops.contains(d))
    {
        return None;
    }
    if let Some(not) = listed("NotShowIn")
        && not.iter().any(|d| env.desktops.contains(d))
    {
        return None;
    }
    if let Some(try_exec) = get("TryExec")
        && !env.on_path(&try_exec)
    {
        return None;
    }

    let name = localized("Name")?;
    let icon = get("Icon").filter(|i| !i.is_empty());
    let argv = expand_exec(&get("Exec")?, &name, icon.as_deref(), path)?;
    let actions = listed("Actions")
        .unwrap_or_default()
        .iter()
        .filter_map(|action| {
            let group = groups.get(&format!("Desktop Action {action}"))?;
            let name = localized_in(group, "Name")?;
            let exec = group.get("Exec").map(|v| unescape(v))?;
            let argv = expand_exec(&exec, &name, icon.as_deref(), path)?;
            Some(DesktopAction { name, argv })
        })
        .collect();

    Some(App {
        id: id.to_owned(),
        name,
        generic_name: localized("GenericName"),
        comment: localized("Comment"),
        keywords: localized("Keywords").map(|k| split_list(&k)).unwrap_or_default(),
        argv,
        terminal: get_bool("Terminal"),
        cwd: get("Path").filter(|p| !p.is_empty()).map(PathBuf::from),
        icon,
        icon_path: None,
        wm_class: get("StartupWMClass").filter(|c| !c.is_empty()),
        actions,
    })
}

/// Raw key/value pairs by group name, such as `Desktop Entry` or
/// `Desktop Action new-window`. The first value of a repeated key wins.
fn groups(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.trim_end().strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.to_owned());
            groups.entry(name.to_owned()).or_default();
            continue;
        }
        if let (Some(group), Some((k, v))) = (&current, line.split_once('=')) {
            groups
                .get_mut(group)
                .expect("group inserted on its header")
                .entry(k.trim().to_owned())
                .or_insert_with(|| v.trim().to_owned());
        }
    }
    groups
}

/// String-value escapes: `\s`, `\n`, `\t`, `\r` and `\\`. `\;` is kept for
/// [`split_list`].
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A `;`-separated list, honouring `\;`.
fn split_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.clone().next() == Some(';') => {
                chars.next();
                cur.push(';');
            }
            ';' => items.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    items.push(cur);
    items
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split `Exec` into arguments and expand field codes for a launch without
/// files or URLs.
pub fn expand_exec(exec: &str, name: &str, icon: Option<&str>, path: &Path) -> Option<Vec<String>> {
    let mut argv = Vec::new();
    for arg in split_exec(exec)? {
        // A field code alone in an argument may expand to nothing or to two
        // arguments; inside a larger argument it expands in place.
        match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => continue,
            "%i" => {
                if let Some(icon) = icon {
                    argv.push("--icon".into());
                    argv.push(icon.into());
                }
                continue;
            }
            _ => {}
        }
        let mut out = String::new();
        let mut chars = arg.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                Some('c') => out.push_str(name),
                Some('k') => out.push_str(&path.to_string_lossy()),
                // Deprecated and file codes expand to nothing.
                Some(_) | None => {}
            }
        }
        argv.push(out);
    }
    (!argv.is_empty()).then_some(argv)
}

/// The spec's `Exec` quoting: whitespace separates arguments; inside double
/// quotes, a backslash escapes `"`, `` ` ``, `$` and `\`.
fn split_exec(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_arg = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_arg = true;
            }
            '\\' if quoted => match chars.next() {
                Some(e @ ('"' | '`' | '$' | '\\')) => cur.push(e),
                Some(other) => {
                    cur.push('\\');
                    cur.push(other);
                }
                None => return None,
            },
            c if c.is_whitespace() && !quoted => {
                if in_arg {
                    args.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            c => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if in_arg {
        args.push(cur);
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            desktops: vec!["niri".into()],
            locales: locale_keys("de_DE.UTF-8"),
            path: vec![],
        }
    }

    fn entry(body: &str) -> Option<App> {
        let text = format!("[Desktop Entry]\nType=Application\n{body}");
        parse("t.desktop", Path::new("/a/t.desktop"), &text, &env())
    }

    #[test]
    fn locale_order() {
        assert_eq!(
            locale_keys("en_GB.UTF-8@euro"),
            ["en_GB@euro", "en_GB", "en@euro", "en"]
        );
        assert_eq!(locale_keys("C"), Vec::<String>::new());
    }

    #[test]
    fn parses_localized_entry() {
        let app = entry(
            "Name=Files\nName[de]=Dateien\nComment=Browse\nKeywords=folder;dir\\;x;\nExec=nautilus --new-window %U\nIcon=org.gnome.Nautilus\n",
        )
        .unwrap();
        assert_eq!(app.name, "Dateien");
        assert_eq!(app.keywords, ["folder", "dir;x"]);
        assert_eq!(app.argv, ["nautilus", "--new-window"]);
        assert_eq!(app.icon.as_deref(), Some("org.gnome.Nautilus"));
    }

    #[test]
    fn desktop_actions() {
        let app = entry(
            "Name=A\nExec=a %u\nActions=new;private;missing;\n[Desktop Action new]\nName=New Window\nName[de]=Neues Fenster\nExec=a --new-window %u\n[Desktop Action private]\nName=Private\nExec=a --private\n",
        )
        .unwrap();
        assert_eq!((app.name.as_str(), app.argv.as_slice()), ("A", ["a".to_string()].as_slice()));
        let actions: Vec<_> = app.actions.iter().map(|a| (a.name.as_str(), a.argv.join(" "))).collect();
        assert_eq!(
            actions,
            [("Neues Fenster", "a --new-window".into()), ("Private", "a --private".into())]
        );
    }

    #[test]
    fn window_ownership() {
        let app = parse(
            "org.mozilla.firefox.desktop",
            Path::new("/f"),
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox\nStartupWMClass=firefox-nightly\n",
            &env(),
        )
        .unwrap();
        assert!(app.owns_window("org.mozilla.firefox"));
        assert!(app.owns_window("Firefox-Nightly"));
        assert!(app.owns_window("firefox"));
        assert!(!app.owns_window("ghostty"));
    }

    #[test]
    fn visibility_rules() {
        assert!(entry("Name=A\nExec=a\nNoDisplay=true").is_none());
        assert!(entry("Name=A\nExec=a\nHidden=true").is_none());
        assert!(entry("Name=A\nExec=a\nOnlyShowIn=GNOME;KDE;").is_none());
        assert!(entry("Name=A\nExec=a\nOnlyShowIn=GNOME;niri;").is_some());
        assert!(entry("Name=A\nExec=a\nNotShowIn=niri;").is_none());
        assert!(entry("Name=A\nExec=a\nTryExec=definitely-not-installed-xyz").is_none());
        let link = parse("l.desktop", Path::new("/l"), "[Desktop Entry]\nType=Link\nName=L\nURL=x", &env());
        assert!(link.is_none());
    }

    #[test]
    fn exec_quoting_and_field_codes() {
        let p = Path::new("/a/t.desktop");
        assert_eq!(
            expand_exec(r#"sh -c "echo \"hi\" \$HOME" %f"#, "N", None, p).unwrap(),
            ["sh", "-c", r#"echo "hi" $HOME"#]
        );
        assert_eq!(
            expand_exec("app %i --name=%c 100%%", "My App", Some("app-icon"), p).unwrap(),
            ["app", "--icon", "app-icon", "--name=My App", "100%"]
        );
        assert_eq!(expand_exec("app %i", "N", None, p).unwrap(), ["app"]);
        assert_eq!(expand_exec(r#"a "" b"#, "N", None, p).unwrap(), ["a", "", "b"]);
        assert!(expand_exec(r#"a "unterminated"#, "N", None, p).is_none());
        assert!(expand_exec("%U", "N", None, p).is_none());
    }

    #[test]
    fn string_escapes() {
        assert_eq!(unescape(r"a\sb\\c\;d"), r"a b\c\;d");
    }
}
