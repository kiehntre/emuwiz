//! Deterministic reading order for archive pages.
//!
//! Rules (all deterministic, none guess):
//! 1. A member at the archive *root* whose file stem is exactly `cover`,
//!    `front`, `frontcover` (ignoring case and non-alphanumerics) is front
//!    matter and sorts first; `back`, `backcover`, `rearcover` sort last.
//!    A `cover.jpg` inside a sub-folder is an ordinary page.
//! 2. Everything else sorts by natural path order: folder by folder, then the
//!    file name, comparing digit runs by numeric value (`2` before `10`, and
//!    `page02` equals `page2` numerically), text case-insensitively, a number
//!    before text where the two differ, a shorter prefix before a longer one.
//! 3. Remaining ties break on the raw name bytes, so the order is total.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageGroup {
    Cover,
    Body,
    BackCover,
}

#[must_use]
pub fn page_group(name: &str) -> PageGroup {
    let normalised = name.replace('\\', "/");
    if normalised.contains('/') {
        return PageGroup::Body;
    }
    let stem = normalised
        .rsplit_once('.')
        .map_or(normalised.as_str(), |(stem, _)| stem);
    let key: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match key.as_str() {
        "cover" | "front" | "frontcover" => PageGroup::Cover,
        "back" | "backcover" | "rearcover" => PageGroup::BackCover,
        _ => PageGroup::Body,
    }
}

#[must_use]
pub fn compare_page_names(a: &str, b: &str) -> Ordering {
    page_group(a)
        .cmp(&page_group(b))
        .then_with(|| natural_path_cmp(a, b))
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

fn natural_path_cmp(a: &str, b: &str) -> Ordering {
    let a = a.replace('\\', "/");
    let b = b.replace('\\', "/");
    let mut left = a.split('/');
    let mut right = b.split('/');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = natural_cmp(x, y);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Number(&'a str),
    Text(&'a str),
}

fn parts(value: &str) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut numeric = None;
    for (index, character) in value.char_indices() {
        let is_digit = character.is_ascii_digit();
        match numeric {
            None => numeric = Some(is_digit),
            Some(previous) if previous != is_digit => {
                out.push(make_part(&value[start..index], previous));
                start = index;
                numeric = Some(is_digit);
            }
            _ => {}
        }
    }
    if let Some(previous) = numeric {
        out.push(make_part(&value[start..], previous));
    }
    out
}

fn make_part(slice: &str, numeric: bool) -> Part<'_> {
    if numeric {
        Part::Number(slice)
    } else {
        Part::Text(slice)
    }
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let left = parts(a);
    let right = parts(b);
    for (x, y) in left.iter().zip(right.iter()) {
        let ord = match (x, y) {
            (Part::Number(p), Part::Number(q)) => number_cmp(p, q),
            (Part::Text(p), Part::Text(q)) => p.to_lowercase().cmp(&q.to_lowercase()),
            (Part::Number(_), Part::Text(_)) => Ordering::Less,
            (Part::Text(_), Part::Number(_)) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    left.len().cmp(&right.len())
}

/// Numeric comparison of digit strings of any length (no overflow).
fn number_cmp(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
pub(super) fn natural_path_cmp_for_tests(a: &str, b: &str) -> Ordering {
    natural_path_cmp(a, b)
}
