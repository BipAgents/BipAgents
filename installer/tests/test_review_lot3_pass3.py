"""Review of lot 3, pass 3 (17/11): C1's deletion (red on 63b86d53a)."""
from pathlib import Path

from bipinstall import moving, vault_keys

from test_vault_keys_lot3 import _exported, offer_place, runtime, write  # noqa: F401


def _import_over(tmp_path, monkeypatch, export_has_mail: bool):
    """An unsealed export (a computer's) imported over a place of the offer sealed at install that already holds the
    person's own mailboxes (made on the place: its agents' addresses), the export without a mail store."""
    source = offer_place(tmp_path / "old" / "alex", sealed_keys=False)
    if not export_has_mail:
        import shutil
        shutil.rmtree(source.data / "mail")
    out, phrase, _ = _exported(source, tmp_path, monkeypatch)
    target_home = tmp_path / "new" / "bip"
    target = offer_place(target_home, sealed_keys=True)
    write(target.data / "connections" / "connections.json.enc", b"the-place-s-own-connections")
    resealed = []
    monkeypatch.setattr(vault_keys, "enable", lambda settings, **kw: resealed.append(kw) or 0)
    tar, info, handle = moving.open_archive(out, phrase)
    try:
        moving.unpack(tar, info, moving.Target(home=target_home, repo=tmp_path / "checkout", tailnet_name=None,
                                               lan_address=None), target.config)
        refused = None
    except Exception as exc:   # an import refused on a place that already holds data is acceptable
        refused = exc
    finally:
        handle.close()
    return target, refused, resealed


def test_an_import_over_a_sealed_place_never_deletes_the_stores_the_export_didn_t_bring(tmp_path, runtime,  # noqa
                                                                                        monkeypatch):
    target, refused, _ = _import_over(tmp_path, monkeypatch, export_has_mail=False)
    mail = target.data / "mail" / "accounts.json.enc"
    connections = target.data / "connections" / "connections.json.enc"
    assert refused or (mail.exists() and connections.exists()), (
        f"deleted for good, not set aside: mail {mail.exists()}, connections {connections.exists()} "
        f"(the place's own, which `bipbridge vault-keys export` could still open before the import)")


def test_the_import_command_never_reaches_an_account_that_has_bipagents(tmp_path, monkeypatch):
    """Context for C1: `bipagents import` (import_main, unpack's only caller) refuses an account with a bridge.toml; an
    uninstall removes ~/.local/share/bipagents. unpack over a sealed place is reached only by a direct call."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    write(tmp_path / ".config" / "bipagents" / "bridge.toml", b'bridge_key = "k"\n')
    archive = tmp_path / "x.bipagents"
    archive.write_bytes(b"x")
    assert moving.import_main([str(archive), "--yes", "--no-tailscale"]) == 1


def test_an_import_that_fails_after_the_sealing_dropped_leaves_the_place_as_it_was(tmp_path, runtime,  # noqa
                                                                                   monkeypatch):
    def broken(*args, **kwargs):
        raise OSError("disk full")
    monkeypatch.setattr(moving, "adjust_bridge", broken)
    target, refused, resealed = _import_over(tmp_path, monkeypatch, export_has_mail=True)
    assert refused is not None
    left = {"keys.sealed.json": (target.data / "keys.sealed.json").exists(),
            "half": (target.space / "vault.share").exists(),
            "own connections": (target.data / "connections" / "connections.json.enc").exists(),
            "resealed": bool(resealed)}
    assert left["keys.sealed.json"] or resealed, f"sealing dropped, nothing resealed, nothing to go back to: {left}"


# -- the author's (third review: the import refused, mkdir by descriptor, the marker never lowered) -------------------

def test_an_import_over_a_sealed_place_is_refused_in_plain_words_and_changes_nothing(tmp_path, runtime,  # noqa: F811
                                                                                    monkeypatch):
    target, refused, resealed = _import_over(tmp_path, monkeypatch, export_has_mail=True)
    from bipinstall.sealed import SealedError
    assert isinstance(refused, SealedError) and "bipagents send" in str(refused), refused
    assert (target.data / "keys.sealed.json").exists() and (target.space / "vault.share").exists()
    assert (target.data / "mail" / "accounts.json.enc").read_bytes() == b"the-mailboxes-bip" and not resealed
    assert not hasattr(vault_keys, "drop_sealing")


def test_the_seat_agent_makes_its_folders_by_descriptor_only(tmp_path, monkeypatch):
    """C4: root never makes a folder by its path either (a parent swapped for a link would put it elsewhere)."""
    import os
    from test_seat_vault_keys import _seat_agent
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    home.mkdir(parents=True)
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    real_mkdir = os.mkdir
    by_path = []

    def mkdir(path, mode=0o777, *, dir_fd=None):
        if dir_fd is None:
            by_path.append(path)
        return real_mkdir(path, mode, dir_fd=dir_fd)
    monkeypatch.setattr(agent.os, "mkdir", mkdir)
    fd, path = agent.place_folder(entry, ".local/state/bipagents")
    os.close(fd)
    assert Path(path).is_dir() and by_path == [], by_path


def test_the_sealed_keys_marker_never_goes_down_from_the_last_release_that_carries_it():
    """Removing or lowering bridge-rs/SEALED_KEYS_VERSION makes every sealed place refuse every update (install.sh)."""
    import subprocess
    root = Path(__file__).resolve().parents[2]
    now = int((root / "bridge-rs" / "SEALED_KEYS_VERSION").read_text().splitlines()[0])
    tags = subprocess.run(["git", "-C", str(root), "tag", "--sort=-creatordate"], capture_output=True, text=True)
    for tag in tags.stdout.split()[:50] if tags.returncode == 0 else []:
        shown = subprocess.run(["git", "-C", str(root), "show", f"{tag}:bridge-rs/SEALED_KEYS_VERSION"],
                               capture_output=True, text=True)
        first = shown.stdout.splitlines()[0].strip() if shown.returncode == 0 and shown.stdout else ""
        if first.isdigit():
            assert now >= int(first), f"lower than {tag}'s: every sealed place would stop updating"
            return

