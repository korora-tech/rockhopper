//! Running packages from nixpkgs: the package list behind the `nix ` mode and
//! the command that runs one.
//!
//! The list is nixpkgs' own `packages.json.br`, published per channel. It is
//! several hundred megabytes unpacked, so it is parsed as a stream into a
//! compact tab-separated cache and refreshed at most once a day.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};

/// How long a downloaded list is used before it is fetched again.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// The attribute path, such as `htop` or `kdePackages.dolphin`.
    pub attr: String,
    pub description: String,
    /// The executable `nix run` starts, when it differs from the attribute.
    pub main_program: Option<String>,
}

pub fn index_url(channel: &str) -> String {
    format!("https://channels.nixos.org/{channel}/packages.json.br")
}

/// Where the compact list for `channel` is kept.
pub fn cache_path(channel: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    // The channel names a file, so it must not name a directory.
    let channel: String = channel
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    base.join("rockhopper")
        .join(format!("nixpkgs-{channel}.tsv"))
}

/// Whether the list at `path` is missing or older than a day.
pub fn is_stale(path: &Path, now: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_none_or(|age| age >= MAX_AGE)
}

/// The command line that runs `attr` from `flake`.
pub fn run_command(flake: &str, attr: &str) -> Vec<String> {
    [
        "nix",
        "--extra-experimental-features",
        "nix-command flakes",
        "run",
    ]
    .into_iter()
    .map(String::from)
    .chain([format!("{flake}#{attr}")])
    .collect()
}

/// Whether a `nix` executable is on the `PATH`.
pub fn is_installed() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("nix").is_file()))
}

/// Read the compact list.
pub fn load(path: &Path) -> io::Result<Vec<Package>> {
    let file = fs::File::open(path)?;
    let mut packages = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let mut fields = line.splitn(3, '\t');
        let (Some(attr), Some(main_program), Some(description)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        packages.push(Package {
            attr: attr.to_owned(),
            description: description.to_owned(),
            main_program: (!main_program.is_empty()).then(|| main_program.to_owned()),
        });
    }
    Ok(packages)
}

fn write(path: &Path, packages: &[Package]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the cache path has no directory"))?;
    fs::create_dir_all(dir)?;
    // Written beside the target and renamed, so a reader never sees half a list.
    let partial = path.with_extension("tsv.partial");
    let mut out = BufWriter::new(fs::File::create(&partial)?);
    let mut line = String::new();
    for package in packages {
        line.clear();
        let _ = writeln!(
            line,
            "{}\t{}\t{}",
            field(&package.attr),
            field(package.main_program.as_deref().unwrap_or("")),
            field(&package.description)
        );
        out.write_all(line.as_bytes())?;
    }
    out.flush()?;
    drop(out);
    fs::rename(&partial, path)
}

/// `text` safe to put in a tab-separated line.
fn field(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Download the list for `channel` and replace the cache at `path`.
pub fn refresh(channel: &str, path: &Path) -> io::Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15 * 60)))
        .build()
        .into();
    let response = agent
        .get(index_url(channel))
        .call()
        .map_err(|err| io::Error::other(format!("could not download the package list: {err}")))?;
    let body = response.into_body().into_reader();
    let json = brotli::Decompressor::new(BufReader::new(body), 64 * 1024);
    let packages = parse(BufReader::with_capacity(256 * 1024, json))?;
    if packages.is_empty() {
        return Err(io::Error::other("the package list had no packages"));
    }
    write(path, &packages)
}

/// The packages in a `packages.json`, read without holding the whole
/// document in memory.
///
/// Broken packages are left out. A package nested in a set (such as
/// `python3Packages.numpy`) is kept only when it names a program to run,
/// since the rest are libraries.
pub fn parse<R: Read>(reader: R) -> io::Result<Vec<Package>> {
    let root: Root = serde_json::from_reader(reader)
        .map_err(|err| io::Error::other(format!("unreadable package list: {err}")))?;
    Ok(root.0)
}

struct Root(Vec<Package>);

impl<'de> Deserialize<'de> for Root {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RootVisitor;
        impl<'de> Visitor<'de> for RootVisitor {
            type Value = Root;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a packages.json object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Root, A::Error> {
                let mut packages = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "packages" {
                        packages = Some(map.next_value::<Packages>()?.0);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                packages
                    .map(Root)
                    .ok_or_else(|| de::Error::missing_field("packages"))
            }
        }
        deserializer.deserialize_map(RootVisitor)
    }
}

struct Packages(Vec<Package>);

impl<'de> Deserialize<'de> for Packages {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PackagesVisitor;
        impl<'de> Visitor<'de> for PackagesVisitor {
            type Value = Packages;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of packages")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Packages, A::Error> {
                let mut packages = Vec::new();
                while let Some(attr) = map.next_key::<String>()? {
                    let raw = map.next_value::<RawPackage>()?;
                    let meta = raw.meta.unwrap_or_default();
                    if meta.broken.as_ref().and_then(|v| v.as_bool()) == Some(true) {
                        continue;
                    }
                    let main_program = meta
                        .main_program
                        .as_ref()
                        .and_then(|v| v.as_str())
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned);
                    if attr.contains('.') && main_program.is_none() {
                        continue;
                    }
                    let description = meta
                        .description
                        .as_ref()
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                    packages.push(Package {
                        // The attribute already names it.
                        main_program: main_program.filter(|name| *name != attr),
                        attr,
                        description,
                    });
                }
                Ok(Packages(packages))
            }
        }
        deserializer.deserialize_map(PackagesVisitor)
    }
}

#[derive(Deserialize)]
struct RawPackage {
    #[serde(default)]
    meta: Option<RawMeta>,
}

/// The few fields kept. They are read as loose values because nixpkgs does not
/// hold every package's metadata to its documented types.
#[derive(Default, Deserialize)]
struct RawMeta {
    description: Option<serde_json::Value>,
    #[serde(rename = "mainProgram")]
    main_program: Option<serde_json::Value>,
    broken: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{
        "version": 2,
        "packages": {
            "htop": {"pname": "htop", "meta": {"description": "Interactive process viewer", "mainProgram": "htop", "homepage": ["a", "b"]}},
            "ripgrep": {"meta": {"description": "Fast grep\n", "mainProgram": "rg"}},
            "zlib": {"meta": {"description": "Compression library"}},
            "broken-thing": {"meta": {"description": "Nope", "broken": true}},
            "python3Packages.numpy": {"meta": {"description": "Arrays"}},
            "kdePackages.dolphin": {"meta": {"description": "Files", "mainProgram": "dolphin"}},
            "no-meta": {}
        }
    }"#;

    #[test]
    fn parses_packages_and_drops_broken_and_nested_libraries() {
        let packages = parse(LISTING.as_bytes()).unwrap();
        let attrs: Vec<_> = packages.iter().map(|p| p.attr.as_str()).collect();
        assert_eq!(
            attrs,
            ["htop", "ripgrep", "zlib", "kdePackages.dolphin", "no-meta"]
        );
        let ripgrep = &packages[1];
        assert_eq!(ripgrep.description, "Fast grep");
        assert_eq!(ripgrep.main_program.as_deref(), Some("rg"));
        // Same as the attribute, so nothing extra to match on.
        assert_eq!(packages[0].main_program, None);
    }

    #[test]
    fn a_document_without_packages_is_an_error() {
        assert!(parse(br#"{"version": 2}"#.as_slice()).is_err());
        assert!(parse(b"nope".as_slice()).is_err());
    }

    #[test]
    fn the_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("rockhopper-nix-{}", std::process::id()));
        let path = dir.join("list.tsv");
        let packages = vec![
            Package {
                attr: "ripgrep".into(),
                description: "Fast\tgrep\nwith odd characters".into(),
                main_program: Some("rg".into()),
            },
            Package {
                attr: "htop".into(),
                description: String::new(),
                main_program: None,
            },
        ];
        write(&path, &packages).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded[0].attr, "ripgrep");
        assert_eq!(loaded[0].description, "Fast grep with odd characters");
        assert_eq!(loaded[0].main_program.as_deref(), Some("rg"));
        assert_eq!(loaded[1].main_program, None);
        assert!(!is_stale(&path, SystemTime::now()));
        assert!(is_stale(&path, SystemTime::now() + MAX_AGE));
        assert!(is_stale(&dir.join("missing.tsv"), SystemTime::now()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn channels_cannot_escape_the_cache_directory() {
        let path = cache_path("../etc/passwd");
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "nixpkgs-.._etc_passwd.tsv"
        );
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "rockhopper");
    }

    #[test]
    fn run_command_names_the_flake_output() {
        assert_eq!(
            run_command("nixpkgs", "htop"),
            [
                "nix",
                "--extra-experimental-features",
                "nix-command flakes",
                "run",
                "nixpkgs#htop"
            ]
        );
    }
}
