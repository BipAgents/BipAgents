//! The layout check's working folder (review of pages-freer-checked, 20/11). The check writes the person's page and
//! its frame under the system's temporary folder, named `bip-page-check-<bridge pid>-<counter from 0>`: on a place
//! where several people have their own Linux account on the same server (installer/cloud/add-seat.sh, one install per
//! session), /tmp is shared and the bridge's pid is public, so another account can make that folder first. It must
//! never be used then: the bridge would write into a folder someone else owns (a symlink `c/f.html` there makes the
//! bridge overwrite any file of its own account), let them read the page (the family's data), or swap the page the
//! browser reads (their text in the agent's tool result). And the folder the bridge makes itself is its own only.
//!
//! Its own test binary: it sets `BIP_PAGE_CHROME` (a stand-in for Chromium that records what it was given) before
//! the first check of the process.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

#[test]
fn the_check_never_writes_into_a_folder_someone_made_first_and_keeps_its_own_private() {
    let scratch = tempfile::tempdir().unwrap();
    // what the bridge's account holds (its config, an agent's key): someone else's symlink points at it
    let victim = scratch.path().join("config.yaml");
    std::fs::write(&victim, "bridge_key: secret\n").unwrap();
    // a Chromium stand-in: notes the mode of the folder and the page it was given, then answers nothing
    let log = scratch.path().join("seen.txt");
    let chrome = scratch.path().join("chrome");
    std::fs::write(
        &chrome,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --user-data-dir=*) d=\"${{a#--user-data-dir=}}\";; esac; done\n\
mode() {{ stat -c %a \"$1\" 2>/dev/null || stat -f %Lp \"$1\"; }}\n\
echo \"dir=$d $(mode \"$d\") page=$(mode \"$d/c/p.html\")\" >> {}\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&chrome, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("BIP_PAGE_CHROME", &chrome);

    // the folders the next checks of this process would take, made first by « someone else », each with a symlink
    let pid = std::process::id();
    let planted: Vec<_> = (0..4).map(|n| std::env::temp_dir().join(format!("bip-page-check-{pid}-{n}"))).collect();
    for dir in &planted {
        std::fs::create_dir_all(dir.join("c")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("c").join("f.html")).unwrap();
    }

    let config = bipbridge::config::Config::with_key("k".repeat(40));
    let (locked, _) =
        bipbridge::pages::lock_with("<h1>Menus de la famille</h1><p>Allergie de Léa : arachide</p>", "Menus", None);
    let found = bipbridge::page_check::check(&config, &locked);
    assert!(found.is_none(), "the stand-in answers nothing");

    let after = std::fs::read_to_string(&victim).unwrap_or_default();
    for dir in &planted {
        let _ = std::fs::remove_dir_all(dir);
    }
    assert_eq!(
        after, "bridge_key: secret\n",
        "the check wrote through someone else's folder into the bridge's own file"
    );
    let seen = std::fs::read_to_string(&log).unwrap_or_default();
    eprintln!("{seen}");
    let line = seen.lines().next().expect("the stand-in ran");
    assert!(line.contains(" 700 "), "the check's folder is readable by others: {line}");
    assert!(
        !planted.iter().any(|dir| line.contains(&format!("dir={} ", dir.display()))),
        "the check used a folder someone else made: {line}"
    );
}
