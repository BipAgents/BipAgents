"""« Documents en ligne » (`/v1/drives…`, bridge-rs/src/drives.rs): Google Drive, OneDrive and Dropbox, folder by
folder. The bridge keeps the keys (never in an answer, never in Hermes), calls the services itself and checks every
item against the folders the person chose for each agent; the agents get read-only tools (drive_folders, drive_list,
drive_search, drive_read, drive_save). The three services are one fake here (Drive v3 under /g, Graph under /ms,
Dropbox under /dbx, one sign-in and token server), started by each test on a port of its own."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit

import httpx

from bbkit.common import AUTH
from bbkit.mcp import TOKEN, call, give_token, rpc, text_of

GOOGLE_FOLDER = "application/vnd.google-apps.folder"


class FakeDrives:
    """Sign-in (/authorize → the app's address, Google's picked ids beside the code), /token, /revoke, and the three
    APIs with a small drive each:

    OneDrive: Docs (notes.txt, Factures/deep.txt), Privé (secret.txt), Partagé (someone else's).
    Dropbox: /work (plan.txt), /workshop (x.txt): a look-alike path.
    Google: Maison (Bail, a Google Doc) and Facture EDF.txt, a file picked on its own; Caché, never picked.
    """

    def __init__(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.seen, self.authorized, self.tokens, self.revoked = [], [], [], []
        self.codes = {}
        self.picked = ""            # what the person picks in Google's window
        self.refuse_refresh = False
        self.slow_once = set()      # paths that answer 429 (Retry-After: 1) once
        self.generation = 0
        self.valid = set()
        self.graph = {
            "ROOT": {"id": "ROOT", "name": "root", "folder": {}},
            "D": {"id": "D", "name": "Docs", "folder": {}, "parentReference": {"id": "ROOT"}},
            "P": {"id": "P", "name": "Privé", "folder": {}, "parentReference": {"id": "ROOT"}},
            "R": {"id": "R", "name": "Partagé", "remoteItem": {"id": "ELSE", "folder": {}},
                  "parentReference": {"id": "ROOT"}},
            "S": {"id": "S", "name": "Factures", "folder": {}, "parentReference": {"id": "D"}},
            "F1": {"id": "F1", "name": "notes.txt", "size": 11, "file": {"mimeType": "text/plain"},
                   "lastModifiedDateTime": "2026-11-02T10:00:00Z", "parentReference": {"id": "D"}},
            "F2": {"id": "F2", "name": "deep.txt", "size": 9, "file": {"mimeType": "text/plain"},
                   "parentReference": {"id": "S"}},
            "X": {"id": "X", "name": "secret.txt", "size": 6, "file": {"mimeType": "text/plain"},
                  "parentReference": {"id": "P"}},
        }
        self.graph_content = {"F1": b"hello notes", "F2": b"deep text", "X": b"secret"}
        self.dropbox = {
            "id:w": {".tag": "folder", "id": "id:w", "name": "Work", "path_lower": "/work"},
            "id:ws": {".tag": "folder", "id": "id:ws", "name": "Workshop", "path_lower": "/workshop"},
            "id:f": {".tag": "file", "id": "id:f", "name": "plan.txt", "size": 4, "path_lower": "/work/plan.txt"},
            "id:z": {".tag": "file", "id": "id:z", "name": "x.txt", "size": 4, "path_lower": "/workshop/x.txt"},
        }
        self.dropbox_content = {"id:f": b"plan", "id:z": b"nope"}
        self.google = {
            "gf": {"id": "gf", "name": "Maison", "mimeType": GOOGLE_FOLDER, "parents": ["groot"]},
            "gd": {"id": "gd", "name": "Bail", "mimeType": "application/vnd.google-apps.document", "parents": ["gf"]},
            "gp": {"id": "gp", "name": "Facture EDF.txt", "mimeType": "text/plain", "parents": ["gx"]},
            "gh": {"id": "gh", "name": "Caché", "mimeType": "text/plain", "parents": ["groot"]},
        }
        # drive.file: only what was picked (and, here, what a picked folder holds)
        self.google_seen = {"gf", "gd", "gp"}
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def issue(self):
        self.generation += 1
        token = f"at-{self.generation}"
        self.valid = {token}
        self.refresh_token = f"rt-{self.generation}"
        return {"access_token": token, "token_type": "bearer", "expires_in": 3600,
                "refresh_token": self.refresh_token}

    def config(self):
        def client(name, api, **more):
            return {"client_id": f"{name}-client", "auth_url": f"{self.base}/authorize",
                    "token_url": f"{self.base}/token", "api_url": f"{self.base}{api}", **more}
        return {
            "enabled": True,
            "google_drive": client("g", "/g", redirect_uri="com.googleusercontent.apps.g-client:/oauth2redirect",
                                   revoke_url=f"{self.base}/revoke"),
            "onedrive": client("ms", "/ms"),
            "dropbox": client("db", "/dbx", revoke_url=f"{self.base}/dbx/auth/token/revoke"),
        }

    def downloaded(self, what):
        return [p for p in self.seen if what in p]

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def send(self, status, body=None, headers=(), raw=None):
                data = raw if raw is not None else (json.dumps(body).encode() if body is not None else b"")
                self.send_response(status)
                self.send_header("content-type", "application/octet-stream" if raw is not None else "application/json")
                self.send_header("content-length", str(len(data)))
                for k, v in headers:
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(data)

            def body(self):
                raw = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
                if "json" in (self.headers.get("content-type") or ""):
                    return json.loads(raw) if raw else {}
                return {k: v[0] for k, v in parse_qs(raw).items()}

            def signed(self):
                auth = self.headers.get("authorization") or ""
                return auth.removeprefix("Bearer ") in fake.valid

            def slowed(self, path):
                if path in fake.slow_once:
                    fake.slow_once.discard(path)
                    self.send(429, {}, [("retry-after", "1")])
                    return True
                return False

            def do_GET(self):
                url = urlsplit(self.path)
                q = {k: v[0] for k, v in parse_qs(url.query).items()}
                fake.seen.append(url.path)
                if url.path == "/authorize":   # the person signs in (Google: and picks in its window)
                    fake.authorized.append(q)
                    code = f"code-{len(fake.authorized)}"
                    fake.codes[code] = q
                    back = {"code": code, "state": q["state"]}
                    if "trigger_onepick" in q:
                        back["picked_file_ids"] = fake.picked
                    return self.send(302, None, [("location", q["redirect_uri"] + "?" + urlencode(back))])
                if not self.signed():
                    return self.send(401, {"error": {"code": "InvalidAuthenticationToken"}})
                if self.slowed(url.path):
                    return None
                # Graph
                if url.path == "/ms/me":
                    return self.send(200, {"mail": "a@example.org"})
                if url.path == "/ms/me/drive/root/children":
                    return self.send(200, {"value": [fake.graph[k] for k in ("P", "D", "R", "F1")]})
                if url.path.startswith("/ms/me/drive/items/"):
                    rest = url.path.removeprefix("/ms/me/drive/items/").split("/", 1)
                    item, what = rest[0], (rest[1] if len(rest) > 1 else "")
                    if item not in fake.graph:
                        return self.send(404, {"error": {"code": "itemNotFound"}})
                    if what == "":
                        return self.send(200, fake.graph[item])
                    if what == "children":
                        kids = [v for v in fake.graph.values() if v.get("parentReference", {}).get("id") == item]
                        return self.send(200, {"value": kids})
                    if what == "content":
                        return self.send(200, raw=fake.graph_content[item])
                    if what.startswith("search("):   # Graph may answer hits from elsewhere: the bridge checks
                        return self.send(200, {"value": [fake.graph["F2"], fake.graph["X"]]})
                # Google
                if url.path == "/g/about":
                    return self.send(200, {"user": {"emailAddress": "g@example.org"}})
                if url.path == "/g/files":
                    # a picked folder's content: nothing (drive.file may not give it)
                    return self.send(200, {"files": []})
                if url.path.startswith("/g/files/"):
                    rest = url.path.removeprefix("/g/files/").split("/", 1)
                    item = rest[0]
                    if item not in fake.google or item not in fake.google_seen:
                        return self.send(404, {"error": {"code": 404, "message": "File not found"}})
                    if len(rest) > 1 and rest[1] == "export":
                        return self.send(200, raw="Le bail commence le 1er mars.".encode())
                    if q.get("alt") == "media":
                        return self.send(200, raw=b"facture edf")
                    return self.send(200, fake.google[item])
                self.send(404, {"error": "not_found", "path": url.path})

            def do_POST(self):
                url = urlsplit(self.path)
                fake.seen.append(url.path)
                body = self.body() if url.path != "/dbx/files/download" else {}
                if url.path == "/token":
                    fake.tokens.append(body)
                    if body.get("grant_type") == "authorization_code":
                        asked = fake.codes.pop(body.get("code"), None)
                        if not asked or asked["redirect_uri"] != body.get("redirect_uri") or "code_verifier" not in body:
                            return self.send(400, {"error": "invalid_grant"})
                        return self.send(200, fake.issue())
                    if body.get("grant_type") == "refresh_token":
                        if fake.refuse_refresh or body.get("refresh_token") != fake.refresh_token:
                            return self.send(400, {"error": "invalid_grant"})
                        return self.send(200, fake.issue())
                    return self.send(400, {"error": "unsupported_grant_type"})
                if url.path in ("/revoke", "/dbx/auth/token/revoke"):
                    fake.revoked.append((url.path, body or self.headers.get("authorization")))
                    return self.send(200, {})
                if not self.signed():
                    return self.send(401, {"error_summary": "expired_access_token/"})
                if self.slowed(url.path):
                    return None
                if url.path == "/dbx/users/get_current_account":
                    return self.send(200, {"email": "d@example.org"})
                if url.path == "/dbx/files/get_metadata":
                    found = fake.dropbox.get(body.get("path"))
                    return self.send(200, found) if found else self.send(409, {"error_summary": "path/not_found/"})
                if url.path == "/dbx/files/list_folder":
                    folder = fake.dropbox.get(body.get("path"), {}).get("path_lower", body.get("path"))
                    kids = [v for v in fake.dropbox.values() if v["path_lower"].rsplit("/", 1)[0] == folder]
                    return self.send(200, {"entries": kids, "has_more": False, "cursor": "c"})
                if url.path == "/dbx/files/search_v2":
                    # every file, whatever the folder: the bridge checks the paths again
                    hits = [{"metadata": {".tag": "metadata", "metadata": v}} for v in fake.dropbox.values()
                            if v[".tag"] == "file"]
                    return self.send(200, {"matches": hits})
                if url.path == "/dbx/files/download":
                    arg = json.loads(self.headers.get("dropbox-api-arg") or "{}")
                    return self.send(200, raw=fake.dropbox_content[arg["path"]])
                self.send(404, {"error": "not_found", "path": url.path})

        return Handler


def install(bridges, fakes, service, enabled=True):
    def change(config):
        config["agents"]["Budget"] = dict(config["agents"]["Wellness"], display_name="Budget",
                                          upload_dir_host=config["agents"]["Wellness"]["upload_dir_host"] + "-budget")
        if service is not None:
            config["drives"] = dict(service.config(), enabled=enabled)
    bridge = bridges.new(change=change)
    bridge.start()
    give_token(bridge)
    return bridge


def connect(bridge, service, drive, agents=(), **more):
    """start → the phone's browser at the service → the app hands the address back."""
    started = bridge.post(f"/v1/drives/{drive}/start", headers=AUTH, json={"agents": list(agents), **more})
    assert started.status_code == 200, started.text
    page = httpx.get(started.json()["authorize_url"], follow_redirects=False)
    assert page.status_code == 302
    back = page.headers["location"]
    done = bridge.post("/v1/drives/callback", headers=AUTH, json={"url": back})
    assert done.status_code == 200, done.text
    return started.json(), done.json()


def tool_names():
    return {"drive_folders", "drive_list", "drive_search", "drive_read", "drive_save"}


def names(bridge):
    return {t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]}


def tool(bridge, name, **arguments):
    result = call(bridge, name, arguments)
    return text_of(result), result["isError"]


def test_off_by_default(bridges, fakes):
    bridge = install(bridges, fakes, None)
    assert bridge.get("/v1/drives", headers=AUTH).status_code == 404
    assert bridge.post("/v1/drives/onedrive/start", headers=AUTH, json={}).status_code == 404
    assert not names(bridge) & tool_names()
    service = FakeDrives()
    try:
        off = install(bridges, fakes, service, enabled=False)
        assert off.get("/v1/drives", headers=AUTH).status_code == 404
    finally:
        service.close()


def test_onedrive_folder_by_folder_for_the_chosen_agent(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        listing = bridge.get("/v1/drives", headers=AUTH)
        assert listing.status_code == 200
        assert [(d["service"], d["configured"], d["state"]) for d in listing.json()["drives"]] == [
            ("google_drive", True, "not_connected"), ("onedrive", True, "not_connected"),
            ("dropbox", True, "not_connected")]
        assert bridge.get("/v1/drives").status_code == 401
        started, shown = connect(bridge, service, "onedrive", client="ios")
        assert started["redirect_uri"] == "bipagents://oauth/callback" and started["expires_in"] == 900
        asked = service.authorized[0]
        assert asked["scope"] == "Files.Read offline_access User.Read" and asked["code_challenge_method"] == "S256"
        assert shown["state"] == "connected" and shown["account"] == "a@example.org" and shown["folders"] == []
        assert not names(bridge) & tool_names(), "connected, no folder given yet"
        # The app's folder choice: sub-folders only, sorted, someone else's folder left out.
        root = bridge.get("/v1/drives/onedrive/browse", headers=AUTH).json()["folders"]
        assert [f["name"] for f in root] == ["Docs", "Privé"]
        inner = bridge.get("/v1/drives/onedrive/browse", headers=AUTH, params={"folder": "D"}).json()["folders"]
        assert [f["name"] for f in inner] == ["Factures"]
        for wrong, code in (("F1", "not_folder"), ("R", "not_folder"), ("NOPE", "not_found")):
            refused = bridge.put("/v1/drives/onedrive/folders", headers=AUTH,
                                 json={"folders": [{"id": wrong, "agents": ["wellness"]}]})
            assert refused.status_code == 422 and refused.json()["detail"]["code"] == code, (wrong, refused.text)
        chosen = bridge.put("/v1/drives/onedrive/folders", headers=AUTH,
                            json={"folders": [{"id": "D", "agents": ["wellness"]}]})
        assert chosen.status_code == 200, chosen.text
        assert chosen.json()["folders"] == [{"id": "D", "name": "Docs", "file": False, "agents": ["wellness"]}]

        assert tool_names() <= names(bridge)
        folders, failed = tool(bridge, "drive_folders")
        assert not failed and "Docs (OneDrive) — onedrive:D" in folders and "Privé" not in folders
        listed, _ = tool(bridge, "drive_list", folder="onedrive:D")
        assert "notes.txt (1 kB, 2026-11-02) — onedrive:F1" in listed and "[folder] Factures" in listed
        deep, failed = tool(bridge, "drive_list", folder="onedrive:S")
        assert not failed and "deep.txt" in deep
        private, failed = tool(bridge, "drive_list", folder="onedrive:P")
        assert failed and "not in a folder your person let you read" in private
        found, _ = tool(bridge, "drive_search", words="texte")
        assert "deep.txt" in found and "secret" not in found
        read, failed = tool(bridge, "drive_read", file="onedrive:F2")
        assert not failed and read.startswith("[The text between these markers comes from your person's online")
        assert "deep text" in read and "page 1/1" in read
        refused, failed = tool(bridge, "drive_read", file="onedrive:X")
        assert failed and not service.downloaded("/items/X/content"), "never downloaded"
        saved, failed = tool(bridge, "drive_save", file="onedrive:F1")
        assert not failed and saved.startswith("Copied: /home/hermes/.hermes/profiles/wellness/uploads/20"), saved
        assert saved.endswith("-notes.txt")
        uploads = bridge.root / "uploads" / "wellness"
        assert [p.read_bytes() for p in uploads.rglob("*-notes.txt")] == [b"hello notes"]
        # Another agent: nothing.
        give_token(bridge, "budget", "budget-token-0123456789abcdefghijkl")
        budget = call(bridge, "drive_read", {"file": "onedrive:F1"}, token="budget-token-0123456789abcdefghijkl")
        assert budget["isError"] and "shared no folder" in text_of(budget)
        # The keys: never in an answer, encrypted on disk.
        everything = json.dumps(bridge.get("/v1/drives", headers=AUTH).json()) + read + saved + folders
        assert "at-1" not in everything and "rt-1" not in everything
        kept = (bridge.state_dir / "connections" / "connections.json.enc").read_bytes()
        assert b"at-1" not in kept and b"rt-1" not in kept and b"Docs" not in kept
        # Disconnected: forgotten here (Microsoft has no revocation for a delegated grant), no tools.
        gone = bridge.delete("/v1/drives/onedrive", headers=AUTH)
        assert gone.status_code == 200 and gone.json() == {"ok": True, "revoked": False}
        assert not names(bridge) & tool_names()
        again = bridge.delete("/v1/drives/onedrive", headers=AUTH)
        assert again.status_code == 404 and again.json()["detail"]["code"] == "not_connected"
    finally:
        service.close()


def test_a_refused_token_is_renewed_a_refused_renewal_ends_it_and_slow_downs_are_waited(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        connect(bridge, service, "onedrive")
        bridge.put("/v1/drives/onedrive/folders", headers=AUTH, json={"folders": [{"id": "D", "agents": ["wellness"]}]})
        # The service stops taking the token before its end: renewed once, the call made again.
        service.valid = set()
        listed, failed = tool(bridge, "drive_list", folder="onedrive:D")
        assert not failed, listed
        assert [t["grant_type"] for t in service.tokens][-1] == "refresh_token"
        # Asked to wait a second: waited, then answered.
        service.slow_once.add("/ms/me/drive/items/D/children")
        listed, failed = tool(bridge, "drive_list", folder="onedrive:D")
        assert not failed and "notes.txt" in listed
        # The renewal refused: the connection ended; the agent says it simply, the app shows « Reconnecter ».
        service.valid = set()
        service.refuse_refresh = True
        ended, failed = tool(bridge, "drive_list", folder="onedrive:D")
        assert failed and "La connexion à OneDrive est terminée" in ended, ended
        drives = bridge.get("/v1/drives", headers=AUTH).json()["drives"]
        assert drives[1]["state"] == "expired" and drives[1]["folders"][0]["name"] == "Docs"
        folders, _ = tool(bridge, "drive_folders")
        assert "connection ended" in folders
        refused = bridge.get("/v1/drives/onedrive/browse", headers=AUTH)
        assert refused.status_code == 409 and refused.json()["detail"]["code"] == "expired"
        # « Reconnecter »: the same account, its folders kept.
        service.refuse_refresh = False
        _, shown = connect(bridge, service, "onedrive")
        assert shown["state"] == "connected" and shown["folders"][0]["agents"] == ["wellness"]
        assert not tool(bridge, "drive_list", folder="onedrive:D")[1]
    finally:
        service.close()


def test_google_keeps_what_was_picked_in_its_window(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        service.picked = "gf,gh-unknown"
        started, shown = connect(bridge, service, "google_drive", agents=["wellness"])
        assert started["redirect_uri"] == "com.googleusercontent.apps.g-client:/oauth2redirect"
        asked = service.authorized[0]
        assert asked["scope"] == "https://www.googleapis.com/auth/drive.file", "never the whole Drive"
        assert asked["trigger_onepick"] == "true" and asked["allow_folder_selection"] == "true"
        assert asked["allow_multiple"] == "true" and asked["prompt"] == "consent"
        assert shown["account"] == "g@example.org"
        assert shown["folders"] == [{"id": "gf", "name": "Maison", "file": False, "agents": ["wellness"]}]
        read, failed = tool(bridge, "drive_read", file="google_drive:gd")
        assert not failed and "Le bail commence le 1er mars." in read
        # A picked folder whose content Google doesn't give: the agent knows what the person can do.
        listed, failed = tool(bridge, "drive_list", folder="google_drive:gf")
        assert not failed and "Ajouter des fichiers" in listed
        # « Ajouter des fichiers »: files only, several; added to what was there.
        service.picked = "gp"
        _, shown = connect(bridge, service, "google_drive", agents=["wellness"], pick="files")
        assert "allow_folder_selection" not in service.authorized[1] and service.authorized[1]["allow_multiple"] == "true"
        assert [(f["id"], f["file"]) for f in shown["folders"]] == [("gf", False), ("gp", True)]
        found, _ = tool(bridge, "drive_search", words="edf")
        assert "Facture EDF.txt — google_drive:gp" in found
        assert "facture edf" in tool(bridge, "drive_read", file="google_drive:gp")[0]
        hidden, failed = tool(bridge, "drive_read", file="google_drive:gh")
        assert failed
        # Only what was picked can be given to an agent; Google's folders are not browsed here.
        refused = bridge.put("/v1/drives/google_drive/folders", headers=AUTH,
                             json={"folders": [{"id": "gh", "agents": ["wellness"]}]})
        assert refused.status_code == 422 and refused.json()["detail"]["code"] == "not_picked"
        assert bridge.get("/v1/drives/google_drive/browse", headers=AUTH).status_code == 422
        # The agents changed in the app (the whole list), one removed.
        kept = bridge.put("/v1/drives/google_drive/folders", headers=AUTH,
                          json={"folders": [{"id": "gp", "agents": ["wellness", "budget"]}]})
        assert kept.status_code == 200 and kept.json()["folders"] == [
            {"id": "gp", "name": "Facture EDF.txt", "file": True, "agents": ["wellness", "budget"]}]
        assert tool(bridge, "drive_read", file="google_drive:gd")[1], "Maison removed"
        # Disconnected: revoked at Google with the renewal token.
        gone = bridge.delete("/v1/drives/google_drive", headers=AUTH)
        assert gone.json() == {"ok": True, "revoked": True}
        assert service.revoked[-1][0] == "/revoke" and service.revoked[-1][1]["token"].startswith("rt-")
    finally:
        service.close()


def test_dropbox_by_path_never_a_look_alike(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        _, shown = connect(bridge, service, "dropbox")
        assert shown["account"] == "d@example.org"
        root = bridge.get("/v1/drives/dropbox/browse", headers=AUTH).json()["folders"]
        assert [(f["id"], f["path"]) for f in root] == [("id:w", "/work"), ("id:ws", "/workshop")]
        bridge.put("/v1/drives/dropbox/folders", headers=AUTH, json={"folders": [{"id": "id:w", "agents": ["wellness"]}]})
        assert "plan" in tool(bridge, "drive_read", file="dropbox:id:f")[0]
        other, failed = tool(bridge, "drive_read", file="dropbox:id:z")
        assert failed, "/workshop is not inside /work"
        found, _ = tool(bridge, "drive_search", words="x")
        assert "plan.txt" in found and "x.txt" not in found
        # The folder renamed at Dropbox: followed by its id.
        service.dropbox["id:w"]["path_lower"] = "/travail"
        service.dropbox["id:f"]["path_lower"] = "/travail/plan.txt"
        assert not tool(bridge, "drive_read", file="dropbox:id:f")[1]
        gone = bridge.delete("/v1/drives/dropbox", headers=AUTH)
        assert gone.json() == {"ok": True, "revoked": True}
        assert service.revoked[-1][0] == "/dbx/auth/token/revoke"
    finally:
        service.close()


def test_what_is_refused(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        unknown = bridge.post("/v1/drives/icloud/start", headers=AUTH, json={})
        assert unknown.status_code == 404
        nobody = bridge.post("/v1/drives/onedrive/start", headers=AUTH, json={"agents": ["nobody"]})
        assert nobody.status_code == 422 and nobody.json()["detail"]["code"] == "agent"
        assert bridge.post("/v1/drives/onedrive/start", headers=AUTH, json={"pick": "everything"}).status_code == 422
        # The person said no; an answer to no sign-in; the same answer twice.
        denied = bridge.post("/v1/drives/callback", headers=AUTH,
                             json={"url": "bipagents://oauth/callback?error=access_denied&state=x"})
        assert denied.status_code == 400 and denied.json()["detail"]["code"] == "denied"
        stray = bridge.post("/v1/drives/callback", headers=AUTH,
                            json={"url": "bipagents://oauth/callback?code=c&state=never"})
        assert stray.status_code == 400 and stray.json()["detail"]["code"] == "unknown_state"
        started = bridge.post("/v1/drives/onedrive/start", headers=AUTH, json={}).json()
        back = httpx.get(started["authorize_url"], follow_redirects=False).headers["location"]
        assert bridge.post("/v1/drives/callback", headers=AUTH, json={"url": back}).status_code == 200
        twice = bridge.post("/v1/drives/callback", headers=AUTH, json={"url": back})
        assert twice.status_code == 400 and twice.json()["detail"]["code"] == "unknown_state"
        # Not connected yet: no folders to set.
        nothing = bridge.put("/v1/drives/dropbox/folders", headers=AUTH, json={"folders": []})
        assert nothing.status_code == 404 and nothing.json()["detail"]["code"] == "not_connected"
        # Android asking Google's way without the Web client on this bridge: the web sign-in instead.
        android = bridge.post("/v1/drives/google_drive/start", headers=AUTH, json={"client": "android"})
        assert android.status_code == 409 and android.json()["detail"]["code"] == "not_configured"
    finally:
        service.close()
