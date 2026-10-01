//! The authored-script schema and its message templates.
//!
//! A script is typed data, not code: a stable ID, what it is about, which facts
//! make it eligible, how important it is, and the authored text at each level.
//! Safety and routing live in these types, never in strings evaluated at runtime.

#![allow(dead_code)]

use super::model::{
    FactKind, GuidanceAction, GuidanceCategory, GuidancePage, GuidanceScope, GuidanceTopic,
    MascotState, Param, Params,
};

/// The only allowed priority values (design section 6). A fixed set, so no
/// provider, adapter or future model can invent an urgency.
pub(crate) const PRIORITY_BANDS: [u8; 8] = [100, 90, 80, 70, 60, 50, 40, 10];

/// Where a script came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptOrigin {
    /// One of the 42 authored design scripts (or a documented variant of one).
    Design,
    /// One of the 35 messages the pre-engine implementation shipped. Kept so the
    /// existing page guidance is unchanged until a page-owned adapter supplies the
    /// design facts that replace it.
    Legacy,
}

/// How often a script may be shown (design section 7). The policy is data; the
/// session memory that applies it is [`super::exposure`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepeatPolicy {
    /// Shown whenever eligible. The pre-engine behaviour of the 35 legacy messages.
    AlwaysShown,
    /// A blocker: stays beside its affected action until resolved. A person may
    /// collapse it, never remove the reason.
    Blocker,
    /// Optional first-use help: once per session and scope, then a compact link;
    /// subject to the optional-tip cooldown.
    FirstUse,
    /// A success: shown until acknowledged for that operation, then suppressed for
    /// the rest of the session. A different operation produces a new message.
    PerOperation,
    /// A non-blocking warning: may be collapsed for the same evidence; a changed
    /// reason shows it again.
    CollapsibleWarning,
}

/// One authored script.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GuidanceScript {
    /// Stable identity; survives any wording change.
    pub(crate) id: &'static str,
    /// 1..=42 for the designed scripts.
    pub(crate) design_number: Option<u8>,
    /// Set on a documented variant of a numbered script (for example the alternate
    /// copy used when only part of the evidence is known).
    pub(crate) variant_of: Option<&'static str>,
    pub(crate) origin: ScriptOrigin,
    pub(crate) category: GuidanceCategory,
    pub(crate) topics: &'static [GuidanceTopic],
    /// Pages the script may appear on; empty means any page.
    pub(crate) pages: &'static [GuidancePage],
    pub(crate) scope: GuidanceScope,
    /// Every one of these fact kinds must be present (and valid).
    pub(crate) requires: &'static [FactKind],
    /// None of these may be present.
    pub(crate) excludes: &'static [FactKind],
    pub(crate) priority: u8,
    /// Scripts that answer the same question share a group (documentation and the
    /// audit; the selector already returns at most one winner).
    pub(crate) exclusive_group: Option<&'static str>,
    pub(crate) repeat: RepeatPolicy,
    pub(crate) mascot: MascotState,
    /// Shortest equivalent of `quick`, for experienced users.
    pub(crate) minimal: Option<&'static str>,
    pub(crate) quick: &'static str,
    pub(crate) explain: Option<&'static str>,
    /// An authored description of the technical details the owning page can show.
    /// It is a sentence, not assembled debug output.
    pub(crate) technical: Option<&'static str>,
    pub(crate) action: Option<GuidanceAction>,
    /// If the fact kind is present, the offered action becomes the given one, or
    /// is omitted for `None`. First match wins.
    pub(crate) action_alternates: &'static [(FactKind, Option<GuidanceAction>)],
    /// For a legacy script: the design scripts that replace it.
    pub(crate) replaced_by: &'static [&'static str],
}

/// Defaults for struct-update syntax in the catalogue. Every real script
/// overrides `id`, `category`, `requires`/`pages` as appropriate and `quick`.
pub(crate) const BASE: GuidanceScript = GuidanceScript {
    id: "",
    design_number: None,
    variant_of: None,
    origin: ScriptOrigin::Design,
    category: GuidanceCategory::Explain,
    topics: &[],
    pages: &[],
    scope: GuidanceScope::Collection,
    requires: &[],
    excludes: &[],
    priority: 10,
    exclusive_group: None,
    repeat: RepeatPolicy::AlwaysShown,
    mascot: MascotState::Helpful,
    minimal: None,
    quick: "",
    explain: None,
    technical: None,
    action: None,
    action_alternates: &[],
    replaced_by: &[],
};

impl GuidanceScript {
    /// Every authored text of the script, in level order.
    pub(crate) fn texts(&self) -> impl Iterator<Item = &'static str> + '_ {
        [self.minimal, Some(self.quick), self.explain, self.technical]
            .into_iter()
            .flatten()
    }
}

// --- Message templates ---------------------------------------------------------------
//
// `{name}` inserts a value. `{name?one|many}` chooses a form by a count (exactly
// one -> `one`, anything else -> `many`), so singular and plural are explicit
// authored text and never "item(s)". Literal braces are not used in messages.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece<'a> {
    Literal(&'a str),
    Value(&'a str),
    Plural {
        name: &'a str,
        one: &'a str,
        many: &'a str,
    },
}

pub(crate) fn parse(template: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            pieces.push(Piece::Literal(&rest[..open]));
        }
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("unclosed placeholder in {template:?}"))?;
        let body = &after[..close];
        if body.contains('{') {
            return Err(format!("nested placeholder in {template:?}"));
        }
        pieces.push(match body.split_once('?') {
            None => Piece::Value(body),
            Some((name, forms)) => {
                let (one, many) = forms
                    .split_once('|')
                    .ok_or_else(|| format!("plural without two forms in {template:?}"))?;
                Piece::Plural { name, one, many }
            }
        });
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err(format!("stray closing brace in {template:?}"));
    }
    if !rest.is_empty() {
        pieces.push(Piece::Literal(rest));
    }
    Ok(pieces)
}

/// Every placeholder in a template: its name and whether it needs a count (a
/// plural form does; a plain value accepts a count or text).
pub(crate) fn placeholders(template: &str) -> Result<Vec<(&str, bool)>, String> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Literal(_) => None,
            Piece::Value(name) => Some((name, false)),
            Piece::Plural { name, .. } => Some((name, true)),
        })
        .collect())
}

/// Renders a template, or `None` if any placeholder has no value of the right
/// kind. A missing value means the script is not eligible: nothing is invented.
pub(crate) fn render(template: &str, params: &Params) -> Option<String> {
    let lookup = |name: &str| params.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
    let mut out = String::new();
    for piece in parse(template).ok()? {
        match piece {
            Piece::Literal(text) => out.push_str(text),
            Piece::Value(name) => match lookup(name)? {
                Param::Count(count) => out.push_str(&count.to_string()),
                Param::Text(text) => out.push_str(text),
            },
            Piece::Plural { name, one, many } => match lookup(name)? {
                Param::Count(1) => out.push_str(one),
                Param::Count(_) => out.push_str(many),
                Param::Text(_) => return None,
            },
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(items: &[(&'static str, Param)]) -> Params {
        items.to_vec()
    }

    #[test]
    fn values_and_plural_forms_render() {
        let p = params(&[
            ("n", Param::Count(1)),
            ("many", Param::Count(3)),
            ("title", Param::Text("Pac-Man".into())),
        ]);
        assert_eq!(
            render("{n} {n?folder|folders} for {title}", &p).unwrap(),
            "1 folder for Pac-Man"
        );
        assert_eq!(
            render("{many} {many?folder|folders}", &p).unwrap(),
            "3 folders"
        );
        assert_eq!(
            render("{many?It is|They are} ready", &p).unwrap(),
            "They are ready"
        );
    }

    #[test]
    fn a_missing_or_wrongly_typed_value_renders_nothing_rather_than_a_guess() {
        let p = params(&[("title", Param::Text("x".into()))]);
        assert_eq!(render("{count} folders", &p), None);
        assert_eq!(render("{title?one|many}", &p), None, "plural needs a count");
    }

    #[test]
    fn malformed_templates_are_rejected_not_half_rendered() {
        for bad in ["{open", "stray}", "{a?one}", "{{a}}"] {
            assert!(parse(bad).is_err(), "{bad}");
            assert_eq!(render(bad, &Vec::new()), None, "{bad}");
        }
    }

    #[test]
    fn placeholders_report_what_each_needs() {
        let found = placeholders("{a} and {b?x|y}").unwrap();
        assert_eq!(found, vec![("a", false), ("b", true)]);
    }
}
