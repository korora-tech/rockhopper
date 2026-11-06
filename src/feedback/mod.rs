//! Send feedback: a draft that becomes a GitHub issue on
//! [`REPO`], or a pre-filled browser page when no account is connected.
//!
//! Nothing here reads launcher state. The draft holds only what the user
//! typed; diagnostics are added only when the user opts in, as exact text.

pub mod composer;
pub mod github;

use std::fmt::Write as _;

/// Where feedback goes. Never configurable, so the destination the composer
/// shows is the destination it uses.
pub const REPO: &str = "korora-tech/rockhopper";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Bug,
    Feature,
    Other,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Bug, Self::Feature, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::Feature => "Feature request",
            Self::Other => "Other",
        }
    }

    /// The title prefix, so triage can sort without labels (which a
    /// reporter without triage rights cannot set).
    fn prefix(self) -> &'static str {
        match self {
            Self::Bug => "bug: ",
            Self::Feature => "feature: ",
            Self::Other => "feedback: ",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub kind: Kind,
    pub title: String,
    pub description: String,
    /// Optional; shown only for bugs.
    pub steps: String,
    /// The user opted in to [`diagnostics`].
    pub include_diagnostics: bool,
}

/// The only environment details ever sent, and only on opt-in. The composer
/// shows exactly this text.
pub fn diagnostics() -> String {
    format!("Rockhopper {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    EmptyTitle,
    EmptyDescription,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::EmptyTitle => "Add a title",
            Self::EmptyDescription => "Add a description",
        })
    }
}

impl Draft {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.description.trim().is_empty()
            && self.steps.trim().is_empty()
    }

    pub fn validate(&self) -> Result<(), Invalid> {
        if self.title.trim().is_empty() {
            return Err(Invalid::EmptyTitle);
        }
        if self.description.trim().is_empty() {
            return Err(Invalid::EmptyDescription);
        }
        Ok(())
    }

    /// The issue title. Single line, so pasted text cannot break the form.
    pub fn issue_title(&self) -> String {
        let title: String = self.title.split_whitespace().collect::<Vec<_>>().join(" ");
        format!("{}{}", self.kind.prefix(), title)
    }

    /// The issue body: the user's text as Markdown, nothing else, except the
    /// opted-in diagnostics.
    pub fn body(&self) -> String {
        let mut body = format!("{}\n", self.description.trim());
        if self.kind == Kind::Bug && !self.steps.trim().is_empty() {
            let _ = write!(body, "\n### Steps to reproduce\n\n{}\n", self.steps.trim());
        }
        if self.include_diagnostics {
            let _ = write!(body, "\n### Diagnostics\n\n{}\n", diagnostics());
        }
        body
    }

    /// Identifies this exact content, for deduplication.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.issue_title(), self.body()).hash(&mut h);
        h.finish()
    }

    /// GitHub's new-issue page with this draft filled in. Used without an
    /// account; the user still has to press "Submit" there.
    pub fn browser_url(&self) -> String {
        format!(
            "https://github.com/{REPO}/issues/new?title={}&body={}",
            percent_encode(&self.issue_title()),
            percent_encode(&self.body()),
        )
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// The token is expired or revoked: reconnect.
    Unauthorized,
    /// The app is not installed on the repository, or lacks the permission.
    Forbidden,
    RateLimited,
    /// No answer, so the issue may or may not exist.
    Network(String),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("GitHub sign-in expired. Reconnect GitHub"),
            Self::Forbidden => write!(
                f,
                "GitHub denied access to {REPO}. Install the app there, or send via browser"
            ),
            Self::RateLimited => f.write_str("GitHub rate limit reached. Try again later"),
            Self::Network(e) => write!(f, "Could not reach GitHub ({e})"),
            Self::Other(e) => write!(f, "GitHub error: {e}"),
        }
    }
}

/// What the submitter needs from GitHub; the real one is
/// [`github::Client`].
pub trait IssueApi {
    /// Create an issue in `repo`. `marker` is embedded in the body so
    /// [`IssueApi::find_by_marker`] can find it.
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<Issue, ApiError>;
    /// An issue in `repo` made by the connected account whose body holds
    /// `marker`.
    fn find_by_marker(&self, repo: &str, marker: &str) -> Result<Option<Issue>, ApiError>;
}

/// Submits drafts so that one confirmation makes at most one issue.
#[derive(Debug, Default)]
pub struct Submitter {
    /// Drafts already published, by fingerprint.
    done: Vec<(u64, Issue)>,
    /// A fingerprint whose last attempt may have reached GitHub.
    uncertain: Option<u64>,
}

impl Submitter {
    /// The marker added to the body: stable for one draft, invisible when
    /// rendered.
    fn marker(fingerprint: u64) -> String {
        format!("<!-- rockhopper-feedback:{fingerprint:016x} -->")
    }

    pub fn submit(&mut self, api: &dyn IssueApi, draft: &Draft) -> Result<Issue, SubmitError> {
        draft.validate().map_err(SubmitError::Invalid)?;
        let fp = draft.fingerprint();
        if let Some((_, issue)) = self.done.iter().find(|(f, _)| *f == fp) {
            return Ok(issue.clone());
        }
        let marker = Self::marker(fp);
        if self.uncertain == Some(fp) {
            // A previous attempt got no answer: look before posting again.
            if let Some(issue) = api
                .find_by_marker(REPO, &marker)
                .map_err(SubmitError::Api)?
            {
                self.uncertain = None;
                self.done.push((fp, issue.clone()));
                return Ok(issue);
            }
        }
        let body = format!("{}\n{marker}\n", draft.body());
        match api.create_issue(REPO, &draft.issue_title(), &body) {
            Ok(issue) => {
                self.uncertain = None;
                self.done.push((fp, issue.clone()));
                Ok(issue)
            }
            Err(err) => {
                if matches!(err, ApiError::Network(_)) {
                    self.uncertain = Some(fp);
                }
                Err(SubmitError::Api(err))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Invalid(Invalid),
    Api(ApiError),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => e.fmt(f),
            Self::Api(e) => e.fmt(f),
        }
    }
}

/// Open `url` in the user's browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests;
