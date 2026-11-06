//! The feedback composer's state: which field is being edited and what
//! pressing `enter` does. No UI here, so it can be tested.

use super::{Draft, Invalid, Issue, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Title,
    Description,
    Steps,
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Editing,
    Sending,
    Sent(Issue),
    /// A message for the user; the draft is untouched.
    Failed(String),
}

/// What the caller must do after [`Composer::enter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Publish through the connected account.
    Send,
    /// Open GitHub's new-issue page with the draft.
    OpenBrowser,
    OpenIssue(String),
    CopyIssue(String),
    /// Finished: close the composer.
    Done,
}

#[derive(Clone, Debug)]
pub struct Composer {
    pub draft: Draft,
    pub stage: Stage,
    pub status: Status,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            draft: Draft::default(),
            stage: Stage::Title,
            status: Status::Editing,
        }
    }
}

impl Composer {
    /// A sent report starts over; anything else resumes.
    pub fn resume(&mut self) {
        if matches!(self.status, Status::Sent(_)) {
            *self = Self::default();
        } else if self.status == Status::Sending {
            // Keep waiting for the answer.
        } else {
            self.status = Status::Editing;
        }
    }

    /// The text the input starts with for the current stage.
    pub fn field(&self) -> String {
        match self.stage {
            Stage::Title => self.draft.title.clone(),
            Stage::Description => self.draft.description.clone(),
            Stage::Steps => self.draft.steps.clone(),
            Stage::Review => String::new(),
        }
    }

    pub fn placeholder(&self) -> &'static str {
        match (self.stage, &self.status) {
            (Stage::Title, _) => "Title: what is this about?",
            (Stage::Description, _) => "Describe the problem or idea…",
            (Stage::Steps, _) => "Steps to reproduce (optional, enter to skip)…",
            (Stage::Review, Status::Sent(_)) => "o open · c copy link · enter done",
            (Stage::Review, _) => "enter send · d diagnostics · b browser · backspace edit",
        }
    }

    /// Keep what was typed in the current field.
    pub fn save(&mut self, text: &str) {
        match self.stage {
            Stage::Title => self.draft.title = text.to_owned(),
            Stage::Description => self.draft.description = text.to_owned(),
            Stage::Steps => self.draft.steps = text.to_owned(),
            Stage::Review => {}
        }
    }

    pub fn cycle_kind(&mut self) {
        if self.status != Status::Sending {
            self.draft.kind = self.draft.kind.next();
        }
    }

    /// Return to the previous field, keeping `text`.
    pub fn back(&mut self, text: &str) {
        if self.status == Status::Sending || matches!(self.status, Status::Sent(_)) {
            return;
        }
        self.save(text);
        self.status = Status::Editing;
        self.stage = match self.stage {
            Stage::Title | Stage::Description => Stage::Title,
            Stage::Steps => Stage::Description,
            Stage::Review if self.draft.kind == Kind::Bug => Stage::Steps,
            Stage::Review => Stage::Description,
        };
    }

    /// Handle `enter` with `text` in the input. `connected` says whether
    /// a GitHub account is available.
    pub fn enter(&mut self, text: &str, connected: bool) -> Next {
        if self.status == Status::Sending {
            return Next::Nothing;
        }
        if matches!(self.status, Status::Sent(_)) {
            return self.review(text.trim(), connected);
        }
        self.save(text);
        self.status = Status::Editing;
        match self.stage {
            Stage::Title => {
                if self.draft.title.trim().is_empty() {
                    self.fail(Invalid::EmptyTitle);
                } else {
                    self.stage = Stage::Description;
                }
                Next::Nothing
            }
            Stage::Description => {
                if self.draft.description.trim().is_empty() {
                    self.fail(Invalid::EmptyDescription);
                } else {
                    self.stage = if self.draft.kind == Kind::Bug {
                        Stage::Steps
                    } else {
                        Stage::Review
                    };
                }
                Next::Nothing
            }
            Stage::Steps => {
                self.stage = Stage::Review;
                Next::Nothing
            }
            Stage::Review => self.review(text.trim(), connected),
        }
    }

    fn review(&mut self, command: &str, connected: bool) -> Next {
        if let Status::Sent(issue) = self.status.clone() {
            return match command {
                "o" => Next::OpenIssue(issue.url),
                "c" => Next::CopyIssue(issue.url),
                _ => Next::Done,
            };
        }
        match command {
            "d" => {
                self.draft.include_diagnostics = !self.draft.include_diagnostics;
                Next::Nothing
            }
            "b" => self.validated(Next::OpenBrowser),
            "" if connected => self.validated(Next::Send),
            "" => self.validated(Next::OpenBrowser),
            _ => Next::Nothing,
        }
    }

    fn validated(&mut self, next: Next) -> Next {
        match self.draft.validate() {
            Ok(()) => {
                if next == Next::Send {
                    self.status = Status::Sending;
                }
                next
            }
            Err(e) => {
                self.fail(e);
                Next::Nothing
            }
        }
    }

    fn fail(&mut self, e: Invalid) {
        self.status = Status::Failed(e.to_string());
    }

    /// Record the outcome of a send. The draft stays on failure.
    pub fn sent(&mut self, result: Result<Issue, String>) {
        self.status = match result {
            Ok(issue) => Status::Sent(issue),
            Err(message) => Status::Failed(message),
        };
        self.stage = Stage::Review;
    }

    /// What the review shows: the exact title and body to be published.
    pub fn review_text(&self) -> (String, String) {
        (self.draft.issue_title(), self.draft.body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_review(kind: Kind) -> Composer {
        let mut c = Composer::default();
        c.draft.kind = kind;
        c.enter("A title", true);
        c.enter("A description", true);
        if kind == Kind::Bug {
            c.enter("", true);
        }
        assert_eq!(c.stage, Stage::Review);
        c
    }

    #[test]
    fn walks_the_stages_and_skips_steps_for_non_bugs() {
        assert_eq!(at_review(Kind::Bug).draft.title, "A title");
        let c = at_review(Kind::Feature);
        assert_eq!(c.draft.description, "A description");
    }

    #[test]
    fn empty_fields_do_not_advance() {
        let mut c = Composer::default();
        c.enter("  ", true);
        assert_eq!(c.stage, Stage::Title);
        assert!(matches!(c.status, Status::Failed(_)));
        c.enter("t", true);
        c.enter("", true);
        assert_eq!(c.stage, Stage::Description);
    }

    #[test]
    fn nothing_is_sent_without_an_explicit_enter_on_review() {
        let mut c = Composer::default();
        assert_eq!(c.enter("t", true), Next::Nothing);
        assert_eq!(c.enter("d", true), Next::Nothing);
        assert_eq!(c.enter("", true), Next::Nothing);
        assert_eq!(c.stage, Stage::Review);
        assert_eq!(c.status, Status::Editing);
    }

    #[test]
    fn review_enter_sends_when_connected_else_opens_browser() {
        let mut c = at_review(Kind::Other);
        assert_eq!(c.enter("", true), Next::Send);
        assert_eq!(c.status, Status::Sending);
        let mut c = at_review(Kind::Other);
        assert_eq!(c.enter("", false), Next::OpenBrowser);
    }

    #[test]
    fn enter_while_sending_does_not_send_twice() {
        let mut c = at_review(Kind::Other);
        assert_eq!(c.enter("", true), Next::Send);
        assert_eq!(c.enter("", true), Next::Nothing);
    }

    #[test]
    fn failure_keeps_the_draft_and_allows_retry() {
        let mut c = at_review(Kind::Bug);
        c.enter("", true);
        c.sent(Err("Could not reach GitHub".into()));
        assert!(matches!(c.status, Status::Failed(_)));
        assert_eq!(c.draft.description, "A description");
        assert_eq!(c.enter("", true), Next::Send);
    }

    #[test]
    fn success_shows_the_link_and_a_new_report_starts_clean() {
        let mut c = at_review(Kind::Other);
        let issue = Issue {
            number: 1,
            url: "https://github.com/korora-tech/rockhopper/issues/1".into(),
        };
        c.sent(Ok(issue.clone()));
        assert_eq!(c.enter("o", true), Next::OpenIssue(issue.url.clone()));
        assert_eq!(c.enter("c", true), Next::CopyIssue(issue.url));
        assert_eq!(c.enter("", true), Next::Done);
        c.resume();
        assert!(c.draft.is_empty());
        assert_eq!(c.stage, Stage::Title);
    }

    #[test]
    fn a_cancelled_draft_is_resumed() {
        let mut c = at_review(Kind::Other);
        c.status = Status::Failed("x".into());
        c.resume();
        assert_eq!(c.draft.title, "A title");
        assert_eq!(c.status, Status::Editing);
    }

    #[test]
    fn diagnostics_toggle_is_opt_in_and_visible_in_review() {
        let mut c = at_review(Kind::Other);
        assert!(!c.review_text().1.contains("Rockhopper "));
        c.enter("d", true);
        assert!(c.review_text().1.contains(&super::super::diagnostics()));
        c.enter("d", true);
        assert!(!c.draft.include_diagnostics);
    }

    #[test]
    fn back_keeps_text() {
        let mut c = at_review(Kind::Bug);
        c.back("");
        assert_eq!(c.stage, Stage::Steps);
        c.back("1. open");
        assert_eq!(c.stage, Stage::Description);
        assert_eq!(c.draft.steps, "1. open");
    }
}
