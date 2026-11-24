"""An agent moving into its own space (bridge-rs/src/spaces.rs, own_space.rs: since 24/11 a project agent only) keeps
what it had going, and a move that doesn't finish is taken up again (token lab of 2026-11-07): the hub's copy deleted
later when Hermes won't delete it yet (« a running gateway still owns this profile », said in the log, not an empty
« failed: »): the move stands, and a space a move left unfinished is removed and the move made again, never « a space
with this name already exists » forever. (Its conversations made in the hub go on in its space: spaces.rs
`rebind_api_sessions`, unit-tested; the pair conversations of a person's agent that moved at its first secret are gone
with that move, 24/11.)

The space's gateway is the fake one (bbkit/fakebin.py `fake-gateway`): it answers runs at once and checks a
conversation's owner record in the profile's state.db as Hermes v0.22.5 does."""
import tomllib

from bbkit.servers import wait_until
from conftest import wait_registered
import test_own_space as own


def test_a_move_whose_hub_delete_fails_stands_and_a_left_space_is_taken_up(bridges, fakes):
    """A project agent leaves the hub at start (own_space.rs `tidy_at_start`). A space an earlier move left
    unfinished is there: removed, the move made again. Hermes won't delete the hub's copy at once: the move stands,
    the log says why, and the delete is made again a moment later."""
    bridge, hub = own.hub_place(bridges, fakes, agents=("coach",), project="fete", tidy_after=1)
    bridge.config["spaces"]["forget_again"] = 0.5
    left = bridge.root / "spaces" / "fete"
    (left / "profiles" / "fete").mkdir(parents=True)
    (left / ".env").write_text("API_SERVER_PORT=1\n")
    (left / "LEFT-BY-AN-EARLIER-MOVE").write_text("x")
    cause = "Error: a running gateway still owns this profile; retry the delete once it settles"
    bridge.commands.rule("hermes", ["profile", "delete", "fete", "--yes"], exit=1, stdout=f"Deleting fete\n{cause}\n",
                         times=1)
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])

    def moved():
        section = tomllib.loads(bridge.toml_text())["agents"].get("fete", {})
        return section.get("isolated") is True and section.get("space") == str(left)
    assert wait_until(moved, timeout=120), bridge.log_tail(4000)
    assert not (left / "LEFT-BY-AN-EARLIER-MOVE").exists()
    assert (left / "profiles" / "fete").is_dir()
    log = bridge.log_tail(6000)
    assert "a space left by a move that didn't finish" in log
    assert "the hub kept the moved agent's profile" in log and cause in log
    deletes = lambda: [e for e in bridge.commands.of("hermes") if e == ["profile", "delete", "fete", "--yes"]]
    assert wait_until(lambda: len(deletes()) >= 2 and not (hub / "profiles" / "fete").is_dir(), timeout=30), \
        bridge.log_tail(4000)
    assert wait_until(lambda: "the hub forgot the moved agent" in bridge.log_tail(6000), timeout=10)
