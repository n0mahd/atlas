//! The report lists the skills a machine has outside Atlas, so the hub can show them.

mod common;

use std::fs;
use std::os::unix::fs::symlink;

use atlas_core::distribution::{OutsideKind, Report};
use atlas_fs::library::read_tree;
use common::{Key, World, write};

#[test]
fn outside_skills_are_reported_with_their_digest_and_atlas_s_are_not() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    write(
        &h.join(".claude/skills/omarchy/SKILL.md"),
        "---\nname: omarchy\n---\n",
    );
    let vendor = w.root.join("usr/share/vendor-skill");
    write(&vendor.join("SKILL.md"), "---\nname: vendor\n---\n");
    symlink(&vendor, h.join(".claude/skills/vendor")).unwrap();
    symlink(w.root.join("gone"), h.join(".claude/skills/gone")).unwrap();
    fs::create_dir_all(h.join(".agents/skills")).unwrap();
    symlink(w.adopt.join("alpha"), h.join(".agents/skills/old")).unwrap();

    let pass = w.apply(1, &key);
    let got: Vec<(&str, &str, OutsideKind)> = pass
        .report
        .outside
        .iter()
        .map(|o| (o.name.as_str(), o.path.as_str(), o.kind))
        .collect();
    assert_eq!(
        got,
        [
            ("gone", "~/.claude/skills/gone", OutsideKind::Broken),
            ("omarchy", "~/.claude/skills/omarchy", OutsideKind::Folder),
            ("vendor", "~/.claude/skills/vendor", OutsideKind::Link),
        ],
        "Atlas's links and adopted ones are not outside"
    );
    let folder = &pass.report.outside[1];
    assert_eq!(
        folder.digest,
        read_tree(&h.join(".claude/skills/omarchy"))
            .unwrap()
            .exact_digest()
    );
    let linked = &pass.report.outside[2];
    assert_eq!(linked.target.as_deref(), Some(vendor.to_str().unwrap()));
    assert!(linked.digest.is_some());
    assert_eq!(pass.report.outside[0].digest, None);

    // A report from an older client, without the list, still reads.
    let old = r#"{"release":1,"at":5,"harnesses":[],"problems":[]}"#;
    let back: Report = serde_json::from_str(old).unwrap();
    assert_eq!(back.outside, Vec::new());
    let json = serde_json::to_string(&pass.report).unwrap();
    assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), pass.report);
}
