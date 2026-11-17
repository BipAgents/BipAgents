"""Review of lot 3, pass 2 (17/11): red on 3e95effa5."""
import json
from pathlib import Path

from bipinstall import moving, vault_keys

from test_vault_keys_lot3 import _exported, offer_place, runtime  # noqa: F401


def test_an_import_of_an_unsealed_export_over_a_sealed_place_is_readable_and_sealed_again(tmp_path, runtime,  # noqa
                                                                                          monkeypatch):
    source = offer_place(tmp_path / "old" / "bip3", sealed_keys=False)       # a computer's, or a place before lot 3
    out, phrase, _ = _exported(source, tmp_path, monkeypatch)
    target_home = tmp_path / "new" / "bip"
    target = offer_place(target_home, sealed_keys=True)                      # a place of the offer, sealed at install
    resealed = []
    monkeypatch.setattr(vault_keys, "enable", lambda settings, **kw: resealed.append(kw) or 0)
    tar, info, handle = moving.open_archive(out, phrase)
    moving.unpack(tar, info, moving.Target(home=target_home, repo=tmp_path / "checkout", tailnet_name=None,
                                           lan_address=None), target.config)
    handle.close()
    data = target.data
    left = {"keys.sealed.json (the place's own)": (data / "keys.sealed.json").exists(),
            "clear vault.key (the person's)": (data / "vault" / "vault.key").exists(),
            "the place's half": (target.space / "vault.share").exists(), "resealed": bool(resealed)}
    assert not (data / "keys.sealed.json").exists(), f"the place's sealing over the person's stores: unreadable {left}"
    assert resealed, left


def test_a_reseal_that_failed_once_is_tried_again_at_the_next_import(tmp_path, monkeypatch):
    """A reseal (restore, move, send, import) is `enable(None)`: its « settings » are always None. One that failed
    (the key service out of reach that day) makes every later one « already tried with these settings »."""
    from types import SimpleNamespace
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(vault_keys, "bridge_command", lambda base=None: ["/bin/bipbridge"])
    (tmp_path / ".config" / "bipagents").mkdir(parents=True)
    (tmp_path / ".config" / "bipagents" / "bridge.toml").write_text('port = 8643\n')
    asked = []

    def runner(code):
        def run(args, env=None):
            if args[0] == "/bin/bipbridge":
                asked.append(args)
            return SimpleNamespace(returncode=code if args[0] == "/bin/bipbridge" else 1, stdout="",
                                   stderr="bipbridge vault-keys enable: the key service can't be reached")
        return run

    assert vault_keys.enable(None, runner=runner(1), say=lambda _: None, base=tmp_path) == 1   # the first reseal
    assert vault_keys.enable(None, runner=runner(0), say=lambda _: None, base=tmp_path) == 0, \
        "the next import's reseal is refused without asking anything: the place stays in clear for good"
    assert len(asked) == 2


def test_the_import_s_reseal_reads_this_place_s_config_not_the_export_s(tmp_path, runtime, monkeypatch):  # noqa
    """`unpack` reseals before it rewrites the export's settings for this machine: the bridge's config then still names
    the source's data folder (another home), which `bipbridge vault-keys enable` refuses (« no data folder »)."""
    source = offer_place(tmp_path / "old" / "bip3")
    out, phrase, _ = _exported(source, tmp_path, monkeypatch)
    target_home = tmp_path / "new" / "bip"
    target = offer_place(target_home, sealed_keys=True)
    seen = []

    def enable(settings, base=None, **kw):
        seen.append(vault_keys.data_dir(base / ".config" / "bipagents" / "bridge.toml", base))
        return 0
    monkeypatch.setattr(vault_keys, "enable", enable)
    tar, info, handle = moving.open_archive(out, phrase)
    moving.unpack(tar, info, moving.Target(home=target_home, repo=tmp_path / "checkout", tailnet_name=None,
                                           lan_address=None), target.config)
    handle.close()
    assert seen == [target.data], f"the reseal would seal {seen}, not this place's {target.data}"


def test_the_seat_agent_never_chowns_through_a_link_swapped_in_after_its_mkdir(tmp_path, monkeypatch):
    """`place_folder` (R6) runs as root: `mkdir` then `chown(path)`, which follows a link. The place's account owns the
    parent, so it can swap the new folder for a link to any file between the two (root's chown then gives it away)."""
    import os
    from test_seat_vault_keys import _seat_agent
    agent = _seat_agent()
    home = tmp_path / "home" / "bip2"
    home.mkdir(parents=True)
    victim = tmp_path / "etc-shadow"
    victim.write_text("root's")
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    real_mkdir = os.mkdir

    def racing_mkdir(path, mode=0o777):
        real_mkdir(path, mode)
        if path.endswith("/.local"):          # the place's own process, at that very moment
            os.rmdir(path)
            os.symlink(victim, path)
    monkeypatch.setattr(agent.os, "mkdir", racing_mkdir)
    chowned = []
    monkeypatch.setattr(agent.os, "chown", lambda path, uid, gid, **kw: chowned.append(
        (os.path.realpath(path) if kw.get("follow_symlinks", True) else str(path))))
    try:
        agent.place_folder(entry, ".local/state/bipagents")
    except OSError:
        pass
    assert str(victim) not in chowned, "root's chown followed the link to another file"


# -- the author's, with the fixes (C3, N3, N1) --------------------------------------------------------------------

def test_a_reseal_order_from_the_guichet_runs_the_place_s_own_key_service(tmp_path, monkeypatch):
    """C3: a place sealed once whose reseal failed (a restore, a send, an import) is sealed again by an order without
    settings (`{"reseal": true}`): its own keyservice.json, no new grant, no settings file."""
    import os
    import time as _time
    from test_review_lot3 import SETTINGS, _seat_agent
    agent = _seat_agent()
    from test_seat_vault_keys import _signed
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    key = Ed25519PrivateKey.generate()
    now = _time.time()
    body = {"order": 41, "server": "vps-1", "seat": 2, "action": "vault_keys", "expires": int(now + 3600),
            "vault_keys": {"reseal": True}}
    assert agent.accepted(_signed(key, body), "vps-1", set(), now) == body
    assert agent.accepted(_signed(key, {**body, "vault_keys": {"reseal": "yes"}}), "vps-1", set(), now) is None
    home = tmp_path / "home" / "bip2"
    (home / ".local" / "state" / "bipagents").mkdir(parents=True)
    monkeypatch.setattr(agent, "VAULT_TRIED", str(tmp_path / "tried"))
    monkeypatch.setattr(agent, "HARDENED", str(tmp_path / "hardened"))
    monkeypatch.setattr(agent, "MOVES", str(tmp_path / "moves"))
    Path(agent.HARDENED).write_text("done")
    entry = type("Entry", (), {"pw_dir": str(home), "pw_uid": os.getuid(), "pw_gid": os.getgid(), "pw_name": "bip2"})
    monkeypatch.setattr(agent.pwd, "getpwnam", lambda name: entry)
    ran = []
    assert agent.vault_keys_order(2, {"reseal": True}, 41, run_as=lambda e, a, timeout=900: ran.append(a) or {}) is True
    assert ran == [["vault-keys", "enable", "--json"]], "no settings file: the place's own key service"
    assert SETTINGS["token"] not in json.dumps(ran)


def test_the_beat_says_sealed_when_the_bridge_is_mute(tmp_path):
    """N3: a sealed place whose bridge doesn't answer is said « sealed » (never taken for one whose sealing failed)."""
    from test_seat_vault_keys import _seat_agent
    agent = _seat_agent()
    home = tmp_path / "bip"
    data = home / ".local" / "share" / "bipagents"
    data.mkdir(parents=True)
    (home / ".config" / "bipagents").mkdir(parents=True)
    (home / ".config" / "bipagents" / "bridge.toml").write_text(f'port = 1\ndata_dir = "{data}"\n')
    assert agent.vault_state(str(home)) == {}
    (data / "keys.sealed.json").write_text("{}")
    assert agent.vault_state(str(home)) == {"vault": "sealed"}


def test_install_sh_s_guard_reads_the_sealed_keys_marker_this_repository_carries():
    """N1: a versioned marker, not file names a refactor may move: install.sh asks for at least the version this
    repository's marker says (a version without it, or older, is refused on a sealed place)."""
    root = Path(__file__).resolve().parents[2]
    marker = root / "bridge-rs" / "SEALED_KEYS_VERSION"
    assert marker.is_file(), "removing it would make every sealed place refuse every update"
    version = int(marker.read_text().strip())
    text = (root / "install.sh").read_text(encoding="utf-8")
    assert "bridge-rs/SEALED_KEYS_VERSION" in text and f"SEALED_KEYS_NEEDED={version}" in text
    assert "hermes_copies.rs" not in text
