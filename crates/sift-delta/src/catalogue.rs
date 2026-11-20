//! `updates.json`, published with every release beside `latest.json`: the
//! deltas among recent releases, from which the app picks its way from the
//! installed version to the newest, and the notes of every release.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalogue {
    /// The release that published it.
    pub version: String,
    /// The size of that release's full package, by operating system as Rust
    /// names it (`macos`, `windows`).
    #[serde(default)]
    pub full: BTreeMap<String, u64>,
    /// The deltas there are, by operating system.
    #[serde(default)]
    pub deltas: BTreeMap<String, Vec<Hop>>,
    /// Every release, newest first.
    #[serde(default)]
    pub history: Vec<Release>,
}

/// A delta from one release to a later one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hop {
    pub from: String,
    pub to: String,
    pub url: String,
    pub size: u64,
    /// Made for version `to`, so a delta cannot be passed off as another.
    pub signature: String,
}

/// One release's section of the changelog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub notes: String,
}

/// The deltas that lead from `current` to `newest` with the least to
/// download, then the fewest steps; none when no deltas lead there, or when
/// they add up to no less than the full package (`full`).
pub fn route(hops: &[Hop], current: &str, newest: &str, full: Option<u64>) -> Vec<Hop> {
    let (Ok(start), Ok(end)) = (Version::parse(current), Version::parse(newest)) else {
        return Vec::new();
    };
    let mut usable: Vec<(Version, Version, &Hop)> = hops
        .iter()
        .filter_map(|hop| Some((Version::parse(&hop.from).ok()?, Version::parse(&hop.to).ok()?, hop)))
        .filter(|(from, to, _)| start <= *from && from < to && *to <= end)
        .collect();
    // Every delta leads to a later release, so by the time those from a
    // release are looked at, the best way to it is known.
    usable.sort_by(|a, b| a.0.cmp(&b.0));
    // The best way found to each release: (bytes, steps, the delta that ends it).
    let mut best: HashMap<&Version, (u64, usize, Option<usize>)> = HashMap::from([(&start, (0, 0, None))]);
    for (i, (from, to, hop)) in usable.iter().enumerate() {
        let Some(&(bytes, steps, _)) = best.get(from) else { continue };
        let way = (bytes.saturating_add(hop.size), steps + 1, Some(i));
        // Of equal ways, the one whose last step starts latest, as the
        // deltas come in from earlier releases first.
        if best.get(to).is_none_or(|known| (way.0, way.1) <= (known.0, known.1)) {
            best.insert(to, way);
        }
    }
    let Some(&(bytes, _, mut last)) = best.get(&end) else { return Vec::new() };
    if full.is_some_and(|full| bytes >= full) {
        return Vec::new();
    }
    let mut path = Vec::new();
    while let Some(i) = last {
        let (from, _, hop) = &usable[i];
        path.push((*hop).clone());
        last = best[from].2;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(from: &str, to: &str, size: u64) -> Hop {
        Hop { from: from.into(), to: to.into(), url: format!("{from}-{to}"), size, signature: String::new() }
    }

    fn names(path: &[Hop]) -> Vec<&str> {
        path.iter().map(|h| h.url.as_str()).collect()
    }

    #[test]
    fn takes_the_least_to_download() {
        // Each release has deltas from the five before it.
        let versions: Vec<String> = (1..=14).map(|n| format!("0.0.{n}")).collect();
        let mut hops = Vec::new();
        for (i, to) in versions.iter().enumerate() {
            for from in &versions[i.saturating_sub(5)..i] {
                hops.push(hop(from, to, 10));
            }
        }
        assert_eq!(names(&route(&hops, "0.0.1", "0.0.14", Some(100))), ["0.0.1-0.0.6", "0.0.6-0.0.11", "0.0.11-0.0.14"]);
        assert_eq!(names(&route(&hops, "0.0.12", "0.0.14", Some(100))), ["0.0.12-0.0.14"]);
        // Three steps of 10 are no smaller than a package of 30.
        assert!(route(&hops, "0.0.1", "0.0.14", Some(30)).is_empty());
        // Nothing leads from a release older than every delta.
        assert!(route(&hops, "0.0.0", "0.0.14", None).is_empty());
    }

    #[test]
    fn prefers_small_deltas_to_few() {
        let hops = [hop("1.0.0", "1.2.0", 50), hop("1.0.0", "1.1.0", 10), hop("1.1.0", "1.2.0", 10), hop("1.1.0", "1.3.0", 5), hop("1.2.0", "1.3.0", 1)];
        assert_eq!(names(&route(&hops, "1.0.0", "1.3.0", None)), ["1.0.0-1.1.0", "1.1.0-1.3.0"]);
        assert_eq!(names(&route(&hops, "1.0.0", "1.2.0", None)), ["1.0.0-1.1.0", "1.1.0-1.2.0"]);
        // Deltas past the newest release, or back to an older one, are not taken.
        let odd = [hop("1.0.0", "1.4.0", 1), hop("1.0.0", "0.9.0", 1), hop("0.9.0", "1.3.0", 1)];
        assert!(route(&odd, "1.0.0", "1.3.0", None).is_empty());
    }

    #[test]
    fn reads_what_the_release_pipeline_writes() {
        let json = r#"{
            "version": "0.6.5",
            "full": { "macos": 200, "windows": 90 },
            "deltas": { "windows": [{ "from": "0.6.4", "to": "0.6.5", "url": "https://x/d", "size": 3, "signature": "s" }] },
            "history": [{ "version": "0.6.5", "date": "2026-12-01", "notes": "- Faster." }, { "version": "0.1.0", "date": null, "notes": "" }]
        }"#;
        let catalogue: Catalogue = serde_json::from_str(json).unwrap();
        assert_eq!(catalogue.full["windows"], 90);
        assert_eq!(names(&route(&catalogue.deltas["windows"], "0.6.4", "0.6.5", Some(90))), ["https://x/d"]);
        assert_eq!(catalogue.history[1].date, None);
    }
}
