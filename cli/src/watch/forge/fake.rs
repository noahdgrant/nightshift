//! A forge in memory, for in-process tests.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use super::{
    ClosingPrs, CommitPr, Filed, Forge, IssueLink, IssueState, MergedPrs, Post, Pr, PrState,
};
use crate::watch::ci::HeadChecks;
use crate::watch::Issue;

/// A forge in memory: issues, the closed ones, and each issue's labels, edited in place.
/// Check logs by where they are; issues filed, as `(title, body, labels)`. Of the closed,
/// those in `not_planned` were closed as not planned. Sub-issues, team comments and the
/// issues a `Part of` search finds by parent; the issues it closed, the comments it posted
/// and the labels it created, in order. A call named in `fails` fails, and so does listing the
/// open issues under a label in `broken_labels`. One fake for every in-process test of `ns watch`
/// and `ns status`.
#[derive(Default)]
pub struct FakeForge {
    pub issues: Vec<Issue>,
    pub closed: BTreeSet<u64>,
    pub not_planned: BTreeSet<u64>,
    pub subs: BTreeMap<u64, Vec<u64>>,
    pub comments: BTreeMap<u64, Vec<String>>,
    pub linked: BTreeMap<u64, Vec<Issue>>,
    pub searches: Cell<usize>,
    pub comment_reads: RefCell<Vec<u64>>,
    pub closes: RefCell<Vec<u64>>,
    pub posted: RefCell<Vec<(u64, String)>>,
    pub created: RefCell<Vec<String>>,
    pub fails: BTreeSet<&'static str>,
    pub broken_labels: BTreeSet<&'static str>,
    pub labels: RefCell<BTreeMap<u64, Vec<String>>>,
    pub main: HeadChecks,
    pub logs: BTreeMap<String, String>,
    pub green: Option<String>,
    pub green_fails: bool,
    pub filed: RefCell<Vec<(String, String, Vec<String>)>>,
}

impl Forge for FakeForge {
    fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
        if self.fails.contains("open_issues") {
            anyhow::bail!("HTTP 502");
        }
        if label.is_some_and(|l| self.broken_labels.contains(l)) {
            anyhow::bail!("gh: not logged in");
        }
        Ok(self
            .issues
            .iter()
            .filter(|i| label.is_none_or(|l| i.labels.iter().any(|x| x == l)))
            .cloned()
            .collect())
    }
    fn labelled_issues(&self, _: &str) -> Result<Vec<Filed>> {
        Ok(Vec::new())
    }
    fn closing_prs(&self, _: PrState) -> Result<ClosingPrs> {
        Ok(ClosingPrs::new())
    }
    fn merged_prs(&self, _: Option<i64>) -> Result<MergedPrs> {
        Ok(MergedPrs::default())
    }
    fn is_closed(&self, n: u64) -> Result<bool> {
        match n {
            99 => anyhow::bail!("unreadable"),
            _ => Ok(self.closed.contains(&n)),
        }
    }
    fn issue_link(&self, _: u64) -> Result<IssueLink> {
        unreachable!("ns watch reads no PR or issue link")
    }
    fn pr(&self, _: u64) -> Result<Pr> {
        unreachable!("ns watch reads no PR or issue link")
    }
    fn commit_prs(&self, _: &str) -> Result<Vec<CommitPr>> {
        unreachable!("ns watch reads no PR or issue link")
    }
    fn state(&self, n: u64) -> Result<IssueState> {
        Ok(match self.is_closed(n)? {
            false => IssueState::Open,
            true if self.not_planned.contains(&n) => IssueState::NotPlanned,
            true => IssueState::Completed,
        })
    }
    fn sub_issues(&self, n: u64) -> Result<Vec<u64>> {
        Ok(self.subs.get(&n).cloned().unwrap_or_default())
    }
    fn team_comments(&self, n: u64) -> Result<Vec<String>> {
        self.comment_reads.borrow_mut().push(n);
        if self.fails.contains("team_comments") {
            anyhow::bail!("HTTP 502");
        }
        Ok(self.comments.get(&n).cloned().unwrap_or_default())
    }
    fn part_of(&self, n: u64) -> Result<Vec<Issue>> {
        self.searches.set(self.searches.get() + 1);
        Ok(self.linked.get(&n).cloned().unwrap_or_default())
    }
    fn ensure_label(&self, label: &str, _: &str) -> Result<()> {
        if self.fails.contains("ensure_label") {
            anyhow::bail!("HTTP 502");
        }
        self.created.borrow_mut().push(label.into());
        Ok(())
    }
    fn labels(&self, n: u64) -> Result<Vec<String>> {
        Ok(self.labels.borrow().get(&n).cloned().unwrap_or_default())
    }
    fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()> {
        if self.fails.contains("edit_labels") {
            anyhow::bail!("HTTP 502");
        }
        let mut all = self.labels.borrow_mut();
        let ls = all.entry(n).or_default();
        ls.retain(|l| !remove.contains(l));
        ls.extend(add.map(String::from));
        Ok(())
    }
    fn comment(&self, n: u64, body: &Post) -> Result<()> {
        if self.fails.contains("comment") {
            anyhow::bail!("HTTP 502");
        }
        self.posted.borrow_mut().push((n, body.as_str().into()));
        Ok(())
    }
    fn close(&self, n: u64) -> Result<()> {
        if self.fails.contains("close") {
            anyhow::bail!("HTTP 502");
        }
        self.closes.borrow_mut().push(n);
        Ok(())
    }
    fn add_pr_label(&self, _: u64, _: &str) -> Result<()> {
        Ok(())
    }
    fn remove_pr_label(&self, _: u64, _: &str) -> Result<()> {
        Ok(())
    }
    fn comment_pr(&self, _: u64, _: &Post) -> Result<()> {
        Ok(())
    }
    fn main_checks(&self) -> Result<HeadChecks> {
        Ok(self.main.clone())
    }
    fn create_issue(&self, title: &str, body: &Post, labels: &[&str]) -> Result<u64> {
        let mut filed = self.filed.borrow_mut();
        let labels = labels.iter().map(|l| l.to_string()).collect();
        filed.push((title.into(), body.as_str().into(), labels));
        Ok(100 + filed.len() as u64 - 1)
    }
    fn check_log(&self, log: &str) -> Result<String> {
        match self.logs.get(log) {
            Some(l) => Ok(l.clone()),
            None => anyhow::bail!("no log at {log}"),
        }
    }
    fn last_green(&self, red: &str) -> Result<Option<String>> {
        if self.green_fails {
            anyhow::bail!("HTTP 502");
        }
        Ok(self.green.clone().filter(|g| g != red))
    }
}
