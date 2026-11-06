use std::cell::RefCell;

use super::*;

fn draft() -> Draft {
    Draft {
        kind: Kind::Bug,
        title: "Crash  on\nopen".into(),
        description: "It crashes.".into(),
        steps: "1. open".into(),
        include_diagnostics: false,
    }
}

#[derive(Default)]
struct Fake {
    created: RefCell<Vec<(String, String, String)>>,
    results: RefCell<Vec<Result<Issue, ApiError>>>,
    existing: RefCell<Option<Issue>>,
}

fn issue(n: u64) -> Issue {
    Issue {
        number: n,
        url: format!("https://github.com/{REPO}/issues/{n}"),
    }
}

impl IssueApi for Fake {
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<Issue, ApiError> {
        self.created
            .borrow_mut()
            .push((repo.into(), title.into(), body.into()));
        self.results.borrow_mut().remove(0)
    }
    fn find_by_marker(&self, _: &str, marker: &str) -> Result<Option<Issue>, ApiError> {
        assert!(marker.contains("rockhopper-feedback"));
        Ok(self.existing.borrow().clone())
    }
}

#[test]
fn title_is_one_line_with_kind() {
    assert_eq!(draft().issue_title(), "bug: Crash on open");
}

#[test]
fn body_holds_only_typed_text() {
    let body = draft().body();
    assert!(body.starts_with("It crashes."));
    assert!(body.contains("Steps to reproduce"));
    assert!(!body.contains("Diagnostics"));
    assert!(!body.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn diagnostics_only_on_opt_in() {
    let mut d = draft();
    d.include_diagnostics = true;
    assert!(d.body().contains(&diagnostics()));
}

#[test]
fn steps_are_for_bugs_only() {
    let mut d = draft();
    d.kind = Kind::Feature;
    assert!(!d.body().contains("Steps"));
}

#[test]
fn validation() {
    let mut d = draft();
    d.title = "  ".into();
    assert_eq!(d.validate(), Err(Invalid::EmptyTitle));
    let mut d = draft();
    d.description.clear();
    assert_eq!(d.validate(), Err(Invalid::EmptyDescription));
    assert!(draft().validate().is_ok());
}

#[test]
fn browser_url_targets_repo_and_encodes() {
    let url = draft().browser_url();
    assert!(url.starts_with("https://github.com/korora-tech/rockhopper/issues/new?title="));
    assert!(url.contains("title=bug%3A%20Crash%20on%20open"));
    assert!(!url.contains(' ') && !url.contains('\n'));
    assert!(!url.contains("&body=&"));
}

#[test]
fn destination_is_always_the_rockhopper_repo() {
    let api = Fake::default();
    api.results.borrow_mut().push(Ok(issue(1)));
    Submitter::default().submit(&api, &draft()).unwrap();
    assert_eq!(api.created.borrow()[0].0, "korora-tech/rockhopper");
}

#[test]
fn one_confirmation_makes_one_issue() {
    let api = Fake::default();
    api.results.borrow_mut().push(Ok(issue(7)));
    let mut s = Submitter::default();
    let first = s.submit(&api, &draft()).unwrap();
    let again = s.submit(&api, &draft()).unwrap();
    assert_eq!(first, again);
    assert_eq!(api.created.borrow().len(), 1);
}

#[test]
fn edited_draft_is_a_new_issue() {
    let api = Fake::default();
    api.results
        .borrow_mut()
        .extend([Ok(issue(1)), Ok(issue(2))]);
    let mut s = Submitter::default();
    s.submit(&api, &draft()).unwrap();
    let mut d = draft();
    d.description = "different".into();
    assert_eq!(s.submit(&api, &d).unwrap().number, 2);
}

#[test]
fn retry_after_lost_response_finds_the_issue_instead_of_duplicating() {
    let api = Fake::default();
    api.results
        .borrow_mut()
        .push(Err(ApiError::Network("timeout".into())));
    let mut s = Submitter::default();
    assert!(s.submit(&api, &draft()).is_err());
    *api.existing.borrow_mut() = Some(issue(9));
    assert_eq!(s.submit(&api, &draft()).unwrap().number, 9);
    assert_eq!(api.created.borrow().len(), 1);
}

#[test]
fn retry_after_lost_response_posts_when_nothing_exists() {
    let api = Fake::default();
    api.results
        .borrow_mut()
        .extend([Err(ApiError::Network("x".into())), Ok(issue(3))]);
    let mut s = Submitter::default();
    assert!(s.submit(&api, &draft()).is_err());
    assert_eq!(s.submit(&api, &draft()).unwrap().number, 3);
    assert_eq!(api.created.borrow().len(), 2);
}

#[test]
fn auth_rate_limit_and_denied_errors_keep_the_draft_retryable() {
    for err in [
        ApiError::Unauthorized,
        ApiError::RateLimited,
        ApiError::Forbidden,
    ] {
        let api = Fake::default();
        api.results
            .borrow_mut()
            .extend([Err(err.clone()), Ok(issue(4))]);
        let mut s = Submitter::default();
        assert_eq!(s.submit(&api, &draft()), Err(SubmitError::Api(err.clone())));
        // Not ambiguous, so no lookup; the retry posts.
        assert_eq!(s.submit(&api, &draft()).unwrap().number, 4);
    }
}

#[test]
fn invalid_draft_is_never_sent() {
    let api = Fake::default();
    let mut d = draft();
    d.title.clear();
    assert!(Submitter::default().submit(&api, &d).is_err());
    assert!(api.created.borrow().is_empty());
}

#[test]
fn feedback_text_is_data_not_a_command() {
    let d = Draft {
        title: "$(rm -rf ~)".into(),
        description: "`reboot`; echo hi".into(),
        ..Default::default()
    };
    // Only ever placed in a URL or request body.
    assert!(d.browser_url().contains("%24%28rm"));
}

#[test]
fn kind_cycles() {
    assert_eq!(Kind::Bug.next(), Kind::Feature);
    assert_eq!(Kind::Other.next(), Kind::Bug);
}
