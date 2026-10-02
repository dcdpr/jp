use pretty_assertions::assert_eq;

use super::*;

/// An official binary JP installed and nobody touched, with a newer release.
fn installed_by_jp() -> Candidate<'static> {
    Candidate {
        official: true,
        location: Location::InstallDir,
        sha256: "old",
        release_sha256: "new",
        pinned: false,
        approval: ApprovalMatch::Matches,
        installed_by_jp: true,
    }
}

#[test]
fn the_binary_jp_installed_is_updated_to_a_new_release() {
    assert_eq!(decide(&installed_by_jp()), Decision::Update);
}

#[test]
fn the_current_release_is_left_as_it_is() {
    let current = Candidate {
        release_sha256: "old",
        ..installed_by_jp()
    };

    assert_eq!(decide(&current), Decision::Current);
}

#[test]
fn a_third_party_plugin_is_not_updated() {
    let third_party = Candidate {
        official: false,
        ..installed_by_jp()
    };

    assert_eq!(decide(&third_party), Decision::Hold(Hold::ThirdParty));
}

#[test]
fn a_pinned_binary_is_not_updated() {
    let pinned = Candidate {
        pinned: true,
        ..installed_by_jp()
    };

    assert_eq!(decide(&pinned), Decision::Hold(Hold::Pinned));
}

/// A package manager's copy on `$PATH` belongs to the package manager.
#[test]
fn a_binary_jp_did_not_install_is_not_updated() {
    let on_path = Candidate {
        location: Location::Path,
        ..installed_by_jp()
    };
    assert_eq!(decide(&on_path), Decision::Hold(Hold::NotInstalledByJp));

    let approved = Candidate {
        installed_by_jp: false,
        ..installed_by_jp()
    };
    assert_eq!(decide(&approved), Decision::Hold(Hold::NotInstalledByJp));
}

/// A binary changed on this machine is someone's work, not a stale release.
#[test]
fn a_binary_changed_since_jp_installed_it_is_not_updated() {
    let changed = Candidate {
        approval: ApprovalMatch::Changed,
        ..installed_by_jp()
    };

    assert_eq!(decide(&changed), Decision::Hold(Hold::Changed));
}

#[test]
fn a_held_plugin_says_why() {
    assert_eq!(
        Hold::Pinned.reason("serve-web"),
        "pinned by plugins.command.serve-web.checksum"
    );
    assert_eq!(
        Hold::ThirdParty.reason("metrics"),
        "third-party plugins are not updated automatically"
    );
}
