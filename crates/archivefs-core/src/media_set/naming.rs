use super::model::*;

fn number(value: &str) -> Option<u16> {
    match value {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        _ => value.parse().ok().filter(|n| *n > 0 && *n <= 4096),
    }
}
fn unit(token: &str, family: Option<MediaFamily>) -> Option<OrdinalUnit> {
    match token {
        "disc" | "cd" => Some(OrdinalUnit::Disc),
        "disk" => Some(OrdinalUnit::Disk),
        "tape" | "cassette" => Some(OrdinalUnit::Tape),
        "part" => Some(OrdinalUnit::Part),
        "reel" => Some(OrdinalUnit::Reel),
        "d" => Some(if family == Some(MediaFamily::Optical) {
            OrdinalUnit::Disc
        } else {
            OrdinalUnit::Disk
        }),
        _ => None,
    }
}
pub(super) fn normalize_title(value: &str) -> String {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
fn role(value: &str) -> Option<MediaRole> {
    match value {
        "game" | "program" => Some(MediaRole::GameMedia),
        "install" | "installation" => Some(MediaRole::InstallMedia),
        "play" => Some(MediaRole::PlayMedia),
        "boot" | "loader" => Some(MediaRole::BootMedia),
        "data" => Some(MediaRole::DataMedia),
        "save" => Some(MediaRole::SaveMedia),
        "bonus" => Some(MediaRole::BonusMedia),
        "extras" | "extra" => Some(MediaRole::ExtrasMedia),
        "demo" => Some(MediaRole::DemoMedia),
        "audio" => Some(MediaRole::AudioMedia),
        "system" => Some(MediaRole::SystemMedia),
        "utility" | "workbench" => Some(MediaRole::UtilityMedia),
        _ => None,
    }
}
/// Bounded token parsing, never fuzzy title matching. All claims remain filename evidence.
pub fn filename_evidence(name: &str, family: Option<MediaFamily>) -> MediaEvidence {
    let mut e = MediaEvidence::new(EvidenceKind::Filename, name);
    if name.len() > 8192 {
        e.notes.push("Filename exceeds topology token limit".into());
        return e;
    }
    // Reuse the reviewed strict DAT classifier before accepting weaker spellings.
    let strict = crate::dat::classification::multidisc_group_key(name);
    let mut tokens = Vec::new();
    let mut ranges = Vec::new();
    let mut start = None;
    let mut digit = false;
    for (offset, c) in name
        .char_indices()
        .chain(std::iter::once((name.len(), ' ')))
    {
        if c.is_alphanumeric() {
            if let Some(begin) = start
                && digit != c.is_ascii_digit()
            {
                tokens.push(name[begin..offset].to_lowercase());
                ranges.push((begin, offset));
                start = Some(offset);
            }
            if start.is_none() {
                start = Some(offset);
            }
            digit = c.is_ascii_digit();
        } else if let Some(begin) = start.take() {
            tokens.push(name[begin..offset].to_lowercase());
            ranges.push((begin, offset));
        }
    }
    let mut used = vec![false; tokens.len()];
    let mut ordinals = Vec::new();
    let mut sides = Vec::new();
    let mut roles = Vec::new();
    let mut counts = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if let Some(u) = unit(t, family)
            && let Some(n) = tokens.get(i + 1).and_then(|s| number(s))
        {
            ordinals.push(MediaOrdinal { number: n, unit: u });
            used[i] = true;
            used[i + 1] = true;
            if tokens.get(i + 2).is_some_and(|x| x == "of")
                && let Some(total) = tokens.get(i + 3).and_then(|s| number(s))
            {
                counts.push(ExpectedCount {
                    count: total,
                    unit: u,
                });
                used[i + 2] = true;
                used[i + 3] = true;
            } else if let Some(total) = tokens.get(i + 2).and_then(|s| number(s))
                && name[ranges[i + 1].1..ranges[i + 2].0].trim() == "-"
            {
                counts.push(ExpectedCount {
                    count: total,
                    unit: u,
                });
                used[i + 2] = true;
            }
        }
        if t == "side"
            && let Some(n) = tokens.get(i + 1).and_then(|s| match s.as_str() {
                "a" | "1" => Some(1),
                "b" | "2" => Some(2),
                _ => None,
            })
        {
            sides.push(MediaSide { number: n });
            used[i] = true;
            used[i + 1] = true;
        }
        if let Some(r) = role(t)
            && (t != "game" || name[..ranges[i].0].trim_end().ends_with(['(', '[']))
            && (tokens.get(i + 1).is_some_and(|x| unit(x, family).is_some())
                || i.checked_sub(1)
                    .is_some_and(|j| unit(&tokens[j], family).is_some()))
        {
            roles.push(r);
            used[i] = true;
            if tokens.get(i + 1).is_some_and(|x| unit(x, family).is_some()) {
                used[i + 1] = true;
            }
        }
        if let Some(n) = number(t)
            && tokens.get(i + 1).is_some_and(|s| s == "of")
            && let Some(total) = tokens.get(i + 2).and_then(|s| number(s))
        {
            let u = ordinals
                .last()
                .map(|o| o.unit)
                .unwrap_or(OrdinalUnit::Medium);
            if !used[i] {
                ordinals.push(MediaOrdinal { number: n, unit: u });
            }
            counts.push(ExpectedCount {
                count: total,
                unit: u,
            });
            used[i] = true;
            used[i + 1] = true;
            used[i + 2] = true;
        }
        let target = match t.as_str() {
            "usa" | "us" => Some((&mut e.variant.region, "usa")),
            "europe" | "eur" | "eu" => Some((&mut e.variant.region, "europe")),
            "japan" | "jpn" | "jp" => Some((&mut e.variant.region, "japan")),
            "world" => Some((&mut e.variant.region, "world")),
            "pal" => Some((&mut e.variant.video_standard, "pal")),
            "ntsc" => Some((&mut e.variant.video_standard, "ntsc")),
            _ => None,
        };
        if let Some((field, value)) = target {
            if field.as_deref().is_some_and(|x| x != value) {
                e.notes
                    .push("Multiple filename region/video variants".into());
            }
            *field = Some(value.into());
            used[i] = true;
        }
        if matches!(t.as_str(), "rev" | "revision")
            && let Some(v) = tokens.get(i + 1)
        {
            e.variant.revision = Some(v.clone());
            used[i] = true;
            used[i + 1] = true;
        }
        if t == "v"
            && let Some(major) = tokens
                .get(i + 1)
                .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
        {
            let mut revision = major.clone();
            used[i] = true;
            used[i + 1] = true;
            let mut j = i + 2;
            while j < tokens.len()
                && tokens[j].chars().all(|c| c.is_ascii_digit())
                && &name[ranges[j - 1].1..ranges[j].0] == "."
            {
                revision.push('.');
                revision.push_str(&tokens[j]);
                used[j] = true;
                j += 1;
            }
            e.variant.revision = Some(revision);
        }
        if let Some(revision) = t.strip_prefix("rev").filter(|s| s.len() == 1) {
            e.variant.revision = Some(revision.into());
            used[i] = true;
        }
        if matches!(t.as_str(), "en" | "fr" | "de" | "es" | "it" | "ja") {
            e.variant
                .language
                .get_or_insert_with(String::new)
                .push_str(&format!("{t},"));
            used[i] = true;
        }
        if matches!(
            t.as_str(),
            "platinum"
                | "budget"
                | "prototype"
                | "proto"
                | "demo"
                | "hack"
                | "translation"
                | "rerelease"
        ) {
            e.variant
                .edition
                .get_or_insert_with(String::new)
                .push_str(&format!("{t},"));
            if role(t).is_none() {
                used[i] = true;
            }
        }
        if t == "greatest" && tokens.get(i + 1).is_some_and(|x| x == "hits") {
            e.variant.edition = Some("greatest hits".into());
            used[i] = true;
            used[i + 1] = true;
        }
    }
    // Only remove the exact delimited range: never consume a sequel number
    // elsewhere in the title just because it equals the count.
    if ordinals.is_empty() {
        for (offset, c) in name.char_indices().filter(|(_, c)| matches!(c, '(' | '[')) {
            let tail = &name[offset + c.len_utf8()..];
            let inner = tail.split([')', ']']).next().unwrap_or("");
            if let Some((a, b)) = inner.split_once('-')
                && let (Some(n), Some(total)) = (number(a.trim()), number(b.trim()))
            {
                ordinals.push(MediaOrdinal {
                    number: n,
                    unit: OrdinalUnit::Medium,
                });
                counts.push(ExpectedCount {
                    count: total,
                    unit: OrdinalUnit::Medium,
                });
                for (i, (start, end)) in ranges.iter().enumerate() {
                    if *start > offset && *end <= offset + 1 + inner.len() {
                        used[i] = true;
                    }
                }
            }
        }
    }
    if let Some(s) = strict {
        if ordinals.is_empty() {
            ordinals.push(MediaOrdinal {
                number: s.part,
                unit: OrdinalUnit::Disc,
            });
        }
        if counts.is_empty() {
            counts.push(ExpectedCount {
                count: s.total,
                unit: OrdinalUnit::Disc,
            });
        }
    }
    ordinals.sort();
    ordinals.dedup();
    counts.sort();
    counts.dedup();
    roles.sort();
    roles.dedup();
    sides.sort();
    sides.dedup();
    if ordinals.len() > 1 {
        e.notes.push("Conflicting filename ordinals".into());
    } else {
        e.ordinal = ordinals.pop();
    }
    if counts.len() > 1 {
        e.notes.push("Conflicting filename counts".into());
    } else {
        e.expected_count = counts.pop();
    }
    if roles.len() > 1 {
        e.notes.push("Conflicting filename roles".into());
    } else {
        e.role = roles.pop();
    }
    if sides.len() > 1 {
        e.notes.push("Conflicting filename sides".into());
    } else {
        e.side = sides.pop();
    }
    if let Some(language) = &e.variant.language {
        e.variant.language = Some(
            language
                .split(',')
                .filter(|s| !s.is_empty())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    e.title = Some(
        tokens
            .into_iter()
            .enumerate()
            .filter_map(|(i, t)| (!used[i]).then_some(t))
            .collect::<Vec<_>>()
            .join(" "),
    );
    e
}
