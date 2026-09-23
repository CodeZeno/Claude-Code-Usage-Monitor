use super::*;
use serde_json::{json, Value};

fn fixture(tag: &str) -> Value {
    let (owner, repo) = github_repo().unwrap();
    json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "assets": [{
            "name": RELEASE_ASSET_NAME,
            "browser_download_url": format!("https://github.com/{owner}/{repo}/releases/download/{tag}/{RELEASE_ASSET_NAME}"),
            "size": 3,
            "digest": "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        }]
    })
}

fn descriptor(value: Value, current: &str) -> Result<Option<ReleaseDescriptor>, String> {
    release_descriptor(serde_json::from_value(value).unwrap(), current)
}

#[test]
fn selects_exact_asset_even_after_an_unrelated_executable() {
    let mut value = fixture("v2.14.0");
    let mut unrelated = value["assets"][0].clone();
    unrelated["name"] = json!("unrelated.exe");
    value["assets"].as_array_mut().unwrap().insert(0, unrelated);
    let release = descriptor(value, "2.13.44").unwrap().unwrap();
    assert_eq!(release.latest_version, "2.14.0");
    assert!(release.asset_url.ends_with(RELEASE_ASSET_NAME));
}

#[test]
fn rejects_missing_wrong_case_and_unrelated_assets() {
    for name in ["other.exe", "CLAUDE-CODE-USAGE-MONITOR.EXE", "app.zip"] {
        let mut value = fixture("v2.14.0");
        value["assets"][0]["name"] = json!(name);
        assert!(descriptor(value, "2.13.44")
            .unwrap_err()
            .contains("missing"));
    }
    let mut value = fixture("v2.14.0");
    value["assets"] = json!([]);
    assert!(descriptor(value, "2.13.44").is_err());
}

#[test]
fn rejects_duplicate_exact_assets() {
    let mut value = fixture("v2.14.0");
    let asset = value["assets"][0].clone();
    value["assets"].as_array_mut().unwrap().push(asset);
    assert!(descriptor(value, "2.13.44")
        .unwrap_err()
        .contains("duplicate"));
}

#[test]
fn rejects_missing_null_and_invalid_digests() {
    for digest in [
        Value::Null,
        json!(""),
        json!("sha256:abcd"),
        json!("sha512:abcd"),
    ] {
        let mut value = fixture("v2.14.0");
        value["assets"][0]["digest"] = digest;
        assert!(descriptor(value, "2.13.44")
            .unwrap_err()
            .contains("SHA-256"));
    }
    let mut value = fixture("v2.14.0");
    value["assets"][0].as_object_mut().unwrap().remove("digest");
    assert!(descriptor(value, "2.13.44").is_err());
}

#[test]
fn rejects_oversized_and_empty_assets() {
    for size in [0, super::super::download::MAX_DOWNLOAD_BYTES + 1] {
        let mut value = fixture("v2.14.0");
        value["assets"][0]["size"] = json!(size);
        assert!(descriptor(value, "2.13.44").unwrap_err().contains("size"));
    }
}

#[test]
fn rejects_unexpected_download_locations() {
    for url in [
        "http://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases/download/v2.14.0/claude-code-usage-monitor.exe",
        "https://example.com/claude-code-usage-monitor.exe",
        "https://github.com/other/repo/releases/download/v2.14.0/claude-code-usage-monitor.exe",
        "https://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases/download/v2.13.44/claude-code-usage-monitor.exe",
    ] {
        let mut value = fixture("v2.14.0");
        value["assets"][0]["browser_download_url"] = json!(url);
        assert!(descriptor(value, "2.13.44").unwrap_err().contains("URL"));
    }
}

#[test]
fn semver_prerelease_precedence_is_preserved() {
    let versions = [
        "2.14.0-alpha",
        "2.14.0-alpha.1",
        "2.14.0-alpha.beta",
        "2.14.0-beta",
        "2.14.0-beta.2",
        "2.14.0-beta.11",
        "2.14.0-rc.1",
        "2.14.0",
    ];
    for pair in versions.windows(2) {
        assert!(parse_version(pair[0])
            .unwrap()
            .cmp_precedence(&parse_version(pair[1]).unwrap())
            .is_lt());
    }
    assert!(parse_version("2.14.0-beta1").unwrap() < parse_version("2.14.0").unwrap());
    assert!(descriptor(fixture("v2.14.0"), "2.14.0-beta1")
        .unwrap()
        .is_some());
}

#[test]
fn skips_prereleases_and_drafts_even_with_mislabelled_tags() {
    assert!(descriptor(fixture("v2.14.0-beta1"), "2.13.44")
        .unwrap()
        .is_none());
    for flag in ["draft", "prerelease"] {
        let mut value = fixture("v2.14.0");
        value[flag] = json!(true);
        assert!(descriptor(value, "2.13.44").unwrap().is_none());
    }
}

#[test]
fn ignores_build_metadata_and_older_or_equal_versions() {
    for (tag, current) in [
        ("v2.14.0+build.2", "2.14.0+build.1"),
        ("v2.14.0", "2.14.0"),
        ("v2.13.44", "2.14.0"),
    ] {
        assert!(descriptor(fixture(tag), current).unwrap().is_none());
    }
    assert!(descriptor(fixture("v2.14.0"), "2.9.99").unwrap().is_some());
}

#[test]
fn parses_numeric_components_without_truncation() {
    for (input, expected) in [
        ("2.13.44", (2, 13, 44)),
        ("v2.13.44", (2, 13, 44)),
        ("4294967296.13.44", (4294967296, 13, 44)),
        (
            "18446744073709551615.18446744073709551615.18446744073709551615",
            (u64::MAX, u64::MAX, u64::MAX),
        ),
    ] {
        let version = parse_version(input).unwrap();
        assert_eq!(
            (version.major, version.minor, version.patch),
            expected,
            "version: {input:?}"
        );
    }
}

#[test]
fn selects_newer_stable_versions_in_numeric_priority_order() {
    for (newer, older) in [
        ("3.0.0", "2.99.99"),
        ("2.14.0", "2.13.99"),
        ("2.13.45", "2.13.44"),
        ("10.0.0", "9.0.0"),
        ("2.10.0", "2.9.0"),
        ("2.13.10", "2.13.9"),
    ] {
        assert!(descriptor(fixture(newer), older).unwrap().is_some());
        assert!(descriptor(fixture(older), newer).unwrap().is_none());
    }
}

#[test]
fn malformed_versions_are_errors() {
    for version in [
        "",
        "invalid",
        "2",
        "2..44",
        ".13.",
        "002.013.044",
        "bad.13.44",
        "2.13.bad",
        "18446744073709551616.13.44",
        "2.18446744073709551616.44",
        "2.13.18446744073709551616",
        "vv2.14.0",
        "2.14",
        "2.14.0.1",
        "2.014.0",
        "2.14.0-",
        "2.x.0",
        "2.14.0-beta.01",
    ] {
        assert!(parse_version(version).is_err(), "{version}");
        assert!(descriptor(fixture(version), "2.13.44").is_err());
        assert!(descriptor(fixture("v2.14.0"), version).is_err());
    }
}
