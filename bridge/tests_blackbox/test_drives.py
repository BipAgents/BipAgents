"""« Documents en ligne » (`/v1/drives…`, bridge-rs/src/drives.rs): Google Drive, OneDrive and Dropbox, folder by
folder. The bridge keeps the keys (never in an answer, never in Hermes), calls the services itself and checks every
item against the folders the person chose for each agent; the agents get read-only tools (drive_folders, drive_list,
drive_search, drive_read, drive_save). The three services are one fake here (Drive v3 under /g, Graph under /ms,
Dropbox under /dbx, one sign-in and token server), started by each test on a port of its own."""
import json
import os
import shlex
import threading
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit

import httpx
import pytest

from bbkit.common import AUTH
from bbkit.mcp import TOKEN, call, give_token, rpc, text_of, not_done
from bbkit.servers import wait_until

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
            "id:f": {".tag": "file", "id": "id:f", "name": "plan.txt", "size": 4, "path_lower": "/work/plan.txt",
                     "rev": "r1"},
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
        self.sessions, self.written, self.deleted, self.made = {}, [], [], 0
        self.ms_sessions, self.links = {}, []
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def issue(self, scope=None):
        self.generation += 1
        token = f"at-{self.generation}"
        self.valid = {token}
        self.refresh_token = f"rt-{self.generation}"
        self.scope = scope if scope is not None else getattr(self, "scope", "")
        answer = {"access_token": token, "token_type": "bearer", "expires_in": 3600, "refresh_token": self.refresh_token}
        if self.scope:
            answer["scope"] = self.scope   # what the person granted (Dropbox: its write right only when asked)
        return answer

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
                    item = "ROOT" if item == "root" else item
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
                if url.path == "/dbx/files/upload":
                    return self.dropbox_upload()
                body = self.body() if url.path != "/dbx/files/download" else {}
                if url.path == "/token":
                    fake.tokens.append(body)
                    if body.get("grant_type") == "authorization_code":
                        asked = fake.codes.pop(body.get("code"), None)
                        if not asked or asked["redirect_uri"] != body.get("redirect_uri") or "code_verifier" not in body:
                            return self.send(400, {"error": "invalid_grant"})
                        return self.send(200, fake.issue(asked.get("scope", "")))
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
                    asked = body.get("path") or ""
                    found = fake.dropbox.get(asked) or next(
                        (v for v in fake.dropbox.values() if v.get("path_lower") == asked.lower()), None)
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
                if url.path == "/dbx/files/delete_v2":
                    found = fake.dropbox.get(body.get("path"))
                    if not found:
                        return self.send(409, {"error_summary": "path_lookup/not_found/"})
                    if found.get("rev") != body.get("parent_rev"):
                        return self.send(409, {"error_summary": "path_write/conflict/"})
                    fake.dropbox.pop(body["path"])
                    fake.deleted.append(body["path"])
                    return self.send(200, {"metadata": found})
                if url.path == "/g/files" and parse_qs(url.query).get("uploadType") == ["resumable"]:
                    return self.google_session(None, body)
                if url.path == "/dbx/sharing/list_shared_links":
                    if "sharing.read" not in fake.scope.split():
                        return self.send(401, {"error_summary": "missing_scope/",
                                               "error": {".tag": "missing_scope", "required_scope": "sharing.read"}})
                    at = (body.get("path") or "").lower()
                    hits = [{".tag": "folder", "url": "https://www.dropbox.com/s/x", "path_lower": p}
                            for p in fake.links if at == p or at.startswith(p + "/")]
                    return self.send(200, {"links": hits, "has_more": False})
                if url.path.startswith("/ms/me/drive/items/") and url.path.endswith(":/createUploadSession"):
                    from urllib.parse import unquote
                    parent, name = url.path.removeprefix("/ms/me/drive/items/").removesuffix(":/createUploadSession").split(":/", 1)
                    name = unquote(name)
                    assert body["item"]["@microsoft.graph.conflictBehavior"] == "fail"
                    if any(v.get("parentReference", {}).get("id") == parent and v["name"] == name for v in fake.graph.values()):
                        return self.send(409, {"error": {"code": "nameAlreadyExists"}})
                    fake.made += 1
                    fake.ms_sessions[f"u{fake.made}"] = (parent, name)
                    host = self.headers.get("host")
                    return self.send(200, {"uploadUrl": f"http://{host}/ms-upload/u{fake.made}"})
                self.send(404, {"error": "not_found", "path": url.path})

            def raw_body(self):
                return self.rfile.read(int(self.headers.get("content-length") or 0))

            def dropbox_upload(self):
                """files/upload: a new file (`add`, never over one) or a new version (`update` on its revision)."""
                fake.seen.append("/dbx/files/upload")
                data = self.raw_body()
                if not self.signed():
                    return self.send(401, {"error_summary": "expired_access_token/"})
                if "files.content.write" not in fake.scope.split():
                    return self.send(401, {"error_summary": "missing_scope/",
                                           "error": {".tag": "missing_scope", "required_scope": "files.content.write"}})
                raw = self.headers.get("dropbox-api-arg") or "{}"
                assert raw.isascii(), raw
                arg = json.loads(raw)
                at = arg["path"]
                old = next((v for v in fake.dropbox.values() if v["path_lower"] == at.lower()), None)
                mode = arg["mode"]
                if mode == "add" and old is None:
                    fake.made += 1
                    new = {".tag": "file", "id": f"id:new{fake.made}", "name": at.rsplit("/", 1)[1],
                           "path_lower": at.lower(), "path_display": at, "rev": "r1", "size": len(data)}
                    fake.dropbox[new["id"]] = new
                elif isinstance(mode, dict) and mode.get(".tag") == "update" and old and old.get("rev") == mode["update"]:
                    new = old
                    new["rev"] = old["rev"] + "+"
                else:
                    return self.send(409, {"error_summary": "path/conflict/file/"})
                fake.dropbox_content[new["id"]] = data
                fake.written.append(("dropbox", new["id"], data))
                return self.send(200, new)

            def google_session(self, file, metadata):
                fake.made += 1
                session = f"s{fake.made}"
                fake.sessions[session] = (file, metadata)
                host = self.headers.get("host")
                return self.send(200, {}, [("location", f"http://{host}/g/upload/{session}")])

            def do_PUT(self):
                url = urlsplit(self.path)
                fake.seen.append(url.path)
                data = self.raw_body()
                if url.path.startswith("/ms-upload/"):   # OneDrive's upload address: never with the bearer
                    assert not self.headers.get("authorization"), "the bearer sent to the upload address"
                    parent, name = fake.ms_sessions.pop(url.path.rsplit("/", 1)[1])
                    fake.made += 1
                    item = {"id": f"M{fake.made}", "name": name, "file": {}, "parentReference": {"id": parent},
                            "eTag": "e1", "cTag": "c1"}
                    fake.graph[item["id"]] = item
                    fake.graph_content[item["id"]] = data
                    fake.written.append(("onedrive", item["id"], data))
                    return self.send(201, item)
                if not self.signed():
                    return self.send(401, {"error": {"code": 401}})
                if url.path.startswith("/g/upload/"):
                    file, metadata = fake.sessions.pop(url.path.rsplit("/", 1)[1])
                    if file is None:
                        fake.made += 1
                        file = f"gnew{fake.made}"
                        fake.google[file] = {"id": file, "name": metadata["name"], "mimeType": metadata["mimeType"],
                                             "parents": metadata["parents"], "headRevisionId": "h1"}
                        fake.google_seen.add(file)
                    else:
                        fake.google[file]["headRevisionId"] = fake.google[file].get("headRevisionId", "h") + "+"
                    fake.written.append(("google_drive", file, data))
                    return self.send(200, fake.google[file])
                if url.path.startswith("/ms/me/drive/items/") and url.path.endswith(":/content"):
                    parent, name = url.path.removeprefix("/ms/me/drive/items/").removesuffix(":/content").split(":/", 1)
                    from urllib.parse import unquote
                    name = unquote(name)
                    if any(v.get("parentReference", {}).get("id") == parent and v["name"] == name for v in fake.graph.values()):
                        return self.send(409, {"error": {"code": "nameAlreadyExists"}})
                    fake.made += 1
                    item = {"id": f"M{fake.made}", "name": name, "file": {}, "parentReference": {"id": parent},
                            "eTag": "e1", "cTag": "c1"}
                    fake.graph[item["id"]] = item
                    fake.graph_content[item["id"]] = data
                    fake.written.append(("onedrive", item["id"], data))
                    return self.send(201, item)
                self.send(404, {"error": "not_found", "path": url.path})

            def do_PATCH(self):
                url = urlsplit(self.path)
                fake.seen.append(url.path)
                body = self.body()
                if not self.signed():
                    return self.send(401, {"error": {"code": 401}})
                item = url.path.removeprefix("/g/files/")
                if "/revisions/" in item:
                    return self.send(200, {"keepForever": True})
                if item not in fake.google:
                    return self.send(404, {"error": {"code": 404}})
                if parse_qs(url.query).get("uploadType") == ["resumable"]:
                    return self.google_session(item, body)
                fake.google[item].update(body)   # « Annuler »: to the bin
                return self.send(200, fake.google[item])

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
    return {"drive_folders", "drive_list", "drive_search", "drive_read", "drive_save", "drive_write"}


def names(bridge):
    return {t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]}


def tool(bridge, name, **arguments):
    """(its text, whether it was refused or failed: a refusal goes out plain, tool_answers.rs)"""
    result = call(bridge, name, arguments)
    return text_of(result), result["isError"] or not_done(result)


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
        assert not_done(budget) and "shared no folder" in text_of(budget)
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
        # Android asking Google's way without the Web client on this bridge: the person chooses first (Kerfy's
        # exchange or their own project), the listing says so
        android = bridge.post("/v1/drives/google_drive/start", headers=AUTH, json={"client": "android"})
        assert android.status_code == 409 and android.json()["detail"]["code"] == "android_choice"
        assert bridge.get("/v1/drives", headers=AUTH).json()["drives"][0]["android"] == "choose"
        own = bridge.put("/v1/drives/google_drive/android", headers=AUTH,
                         json={"mode": "own", "client_id": "mine.apps.googleusercontent.com", "client_secret": "s"})
        assert own.status_code == 200 and own.json() == {"android": "own"}
        assert "\"s\"" not in bridge.get("/v1/drives", headers=AUTH).text
        started = bridge.post("/v1/drives/google_drive/start", headers=AUTH, json={"client": "android"}).json()
        assert started["server_client_id"] == "mine.apps.googleusercontent.com"
        gone = bridge.delete("/v1/drives/google_drive/android", headers=AUTH)
        assert gone.status_code == 200 and gone.json()["android"] == "choose"
    finally:
        service.close()


# -- writing (drives_write.rs, Alex 23/11): a folder's switch, the person's card or level, « Annuler » -------------

def cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-ok-")]


def write_answered(bridge, choice, meanwhile=None, **arguments):
    """drive_write while the person answers its card (`meanwhile`: what they do before answering)."""
    with ThreadPoolExecutor(1) as pool:
        result = pool.submit(call, bridge, "drive_write", arguments)
        assert wait_until(lambda: cards(bridge), timeout=30), "no card"
        [card] = cards(bridge)
        if meanwhile:
            meanwhile()
        said = bridge.post("/v1/approve", headers=AUTH,
                           json={"agent": "wellness", "run_id": card["run_id"], "choice": choice})
        assert said.status_code == 200, said.text
        return result.result(timeout=60), card


def write_tool(bridge, **arguments):
    """drive_write without a card answered: (its text, whether it was refused); `name` is one of its arguments."""
    result = call(bridge, "drive_write", arguments)
    return text_of(result), result["isError"] or not_done(result)


def folders_put(bridge, drive, rows):
    put = bridge.put(f"/v1/drives/{drive}/folders", headers=AUTH, json={"folders": rows})
    assert put.status_code == 200, put.text
    return put.json()


def test_dropbox_writes_after_the_persons_yes_only_where_its_switch_is_on(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        _, shown = connect(bridge, service, "dropbox")
        assert "files.content.write" not in service.authorized[-1]["scope"], "reading only: no write right asked"
        assert shown["writable"] is False and shown["whole_drive"] is True
        shown = folders_put(bridge, "dropbox", [{"id": "id:w", "agents": ["wellness"]}])
        assert shown["folders"] == [{"id": "id:w", "name": "Work", "file": False, "agents": ["wellness"]}], "1.0's shape"
        # the switch off: the agent says how to turn it on, nothing asked
        said, failed = write_tool(bridge, folder="dropbox:id:w", name="Compte rendu", text="# Réunion")
        assert failed and "peut y écrire" in said and not cards(bridge), said
        # on, but Dropbox hasn't given the right yet: the person reconnects once (its folders and switches kept)
        shown = folders_put(bridge, "dropbox", [{"id": "id:w", "agents": ["wellness"], "writers": ["wellness"]}])
        assert shown["folders"][0]["writers"] == ["wellness"] and shown["writable"] is False
        said, failed = write_tool(bridge, folder="dropbox:id:w", name="Compte rendu", text="# Réunion")
        assert failed and "right to write" in said and not cards(bridge), said
        _, shown = connect(bridge, service, "dropbox")
        assert service.authorized[-1]["scope"].endswith(" files.content.write sharing.read"), "its links read too"
        assert shown["writable"] is True and shown["folders"][0]["writers"] == ["wellness"]
        # « Oui »: written; the card says what and where
        result, card = write_answered(bridge, "once", folder="dropbox:id:w", name="Compte rendu", text="# Réunion")
        assert card["bip_ok"]["kind"] == "drive_write" and card["choices"] == ["once", "deny"], card
        assert card["bip_summary"] in ("Save « Compte rendu.md » in Dropbox › Work?",
                                       "Enregistrer « Compte rendu.md » dans Dropbox › Work ?"), card
        assert card["bip_detail"] == "# Réunion"
        said = text_of(result)
        assert not result["isError"] and said.startswith("Saved, a new file: dropbox:id:new"), result
        # the names from the service between the markers that say they are data (review A-marqueurs)
        assert "[The text between these markers" in said and "« Compte rendu.md » in Dropbox › Work" in said, said
        assert service.written[-1][2] == "# Réunion".encode()
        new_id = service.written[-1][1]
        assert service.dropbox[new_id]["path_lower"] == "/work/compte rendu.md"
        # never over a file: the same name again is refused before any card
        said, failed = write_tool(bridge, folder="dropbox:id:w", name="Compte rendu.md", text="x")
        assert failed and "exists already" in said and not cards(bridge), said
        # « Non »: nothing
        before = len(service.written)
        result, _ = write_answered(bridge, "deny", folder="dropbox:id:w", name="Autre.md", text="x")
        assert not_done(result) or result["isError"]
        assert len(service.written) == before
        # a new version: never over a change since the agent read it (`seen`, the revision drive_read shows)
        read, _ = tool(bridge, "drive_read", file="dropbox:id:f")
        assert "(revision r1:" in read, read
        said, failed = write_tool(bridge, replace="dropbox:id:f", text="x", seen="r0")
        assert failed and "changed in Dropbox since you read it" in said and not cards(bridge), said
        # a new version: on the revision read
        result, card = write_answered(bridge, "once", replace="dropbox:id:f", text="plan v2", seen="r1")
        assert card["bip_ok"]["kind"] == "drive_replace", card
        assert service.dropbox_content["id:f"] == b"plan v2" and service.dropbox["id:f"]["rev"] != "r1"
        # never outside: the look-alike, a path for a name
        said, failed = write_tool(bridge, folder="dropbox:id:ws", name="x.md", text="x")
        assert failed and not cards(bridge), said
        said, failed = write_tool(bridge, folder="dropbox:id:w", name="../x.md", text="x")
        assert failed and "path" in said and not cards(bridge), said
        # the folder taken back while the card waits: nothing written
        before = len(service.written)
        result, _ = write_answered(bridge, "once", folder="dropbox:id:w", name="Tard.md", text="x",
                                   meanwhile=lambda: folders_put(bridge, "dropbox", []))
        assert len(service.written) == before and (result["isError"] or not_done(result)), result
    finally:
        service.close()


def test_seulement_le_critique_saves_a_new_file_at_once_and_annuler_puts_it_in_the_bin(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        connect(bridge, service, "dropbox")
        folders_put(bridge, "dropbox", [{"id": "id:w", "agents": ["wellness"], "writers": ["wellness"]}])
        connect(bridge, service, "dropbox")   # the write right
        assert bridge.put("/v1/approval-level", headers=AUTH, json={"level": "critical"}).status_code == 200
        result = call(bridge, "drive_write", {"folder": "dropbox:id:w", "name": "Notes", "text": "à garder"})
        assert not result["isError"] and "Done at once" in text_of(result) and not cards(bridge), result
        new_id = service.written[-1][1]
        lines = bridge.get("/v1/approvals/auto", headers=AUTH, params={"agent": "wellness"}).json()["items"]
        [line] = [l for l in lines if l["kind"] == "drive_write"]
        assert line["undo"] is True and line["summary"] in ("« Notes.md » saved in Dropbox › Work",
                                                            "« Notes.md » enregistré dans Dropbox › Work"), line
        undone = bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH)
        assert undone.status_code == 200, undone.text
        assert new_id in service.deleted and new_id not in service.dropbox
        # never a type that runs (review B1), card or not
        said, failed = write_tool(bridge, folder="dropbox:id:w", name="Facture.pdf.bat", text="@echo off")
        assert failed and "never written" in said and not cards(bridge), said
        # not a document or a picture: a card, whatever the level
        result, card = write_answered(bridge, "deny", folder="dropbox:id:w", name="page.html", text="<p>x</p>")
        assert card["bip_ok"]["kind"] == "drive_write", card
        # a new version stays asked, whatever the level
        result, card = write_answered(bridge, "once", replace="dropbox:id:f", text="v2")
        assert card["bip_ok"]["kind"] == "drive_replace" and not result["isError"], result
    finally:
        service.close()


def test_google_and_onedrive_write_and_the_whole_drive(bridges, fakes):
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        service.picked = "gf,gp"
        _, shown = connect(bridge, service, "google_drive", agents=["wellness"])
        assert shown["writable"] is True and shown["whole_drive"] is False
        assert service.authorized[-1]["scope"] == "https://www.googleapis.com/auth/drive.file", "never wider"
        whole = bridge.put("/v1/drives/google_drive/folders", headers=AUTH,
                           json={"folders": [{"id": "root", "agents": ["wellness"]}]})
        assert whole.status_code == 422 and whole.json()["detail"]["code"] == "whole_drive"
        folders_put(bridge, "google_drive", [{"id": "gf", "agents": ["wellness"], "writers": ["wellness"]},
                                             {"id": "gp", "agents": ["wellness"]}])
        result, card = write_answered(bridge, "once", folder="google_drive:gf", name="Liste.md", text="- pain")
        assert not result["isError"], result
        made = service.written[-1]
        assert made[0] == "google_drive" and service.google[made[1]]["parents"] == ["gf"] and made[2] == b"- pain"
        said, failed = write_tool(bridge, replace="google_drive:gp", text="x")
        assert failed and "peut y écrire" in said, "its own switch, off"
        # OneDrive: Files.ReadWrite, a new file with its card, then the whole OneDrive read from its root
        _, shown = connect(bridge, service, "onedrive")
        assert service.authorized[-1]["scope"] == "Files.Read offline_access User.Read", "reading until a writer (C6)"
        shown = folders_put(bridge, "onedrive", [{"id": "D", "agents": ["wellness"], "writers": ["wellness"]}])
        assert shown["writable"] is False
        said, failed = write_tool(bridge, folder="onedrive:D", name="Été.md", text="x")
        assert failed and "right to write" in said and not cards(bridge), said
        _, shown = connect(bridge, service, "onedrive")
        assert service.authorized[-1]["scope"] == "Files.ReadWrite offline_access User.Read" and shown["writable"] is True
        result, card = write_answered(bridge, "once", folder="onedrive:D", name="Été.md", text="x")
        assert not result["isError"] and service.written[-1][0] == "onedrive", result
        assert service.graph[service.written[-1][1]]["name"] == "Été.md"
        shown = folders_put(bridge, "onedrive", [{"id": "root", "agents": ["wellness"]}])
        assert shown["folders"] == [{"id": "root", "name": "OneDrive", "file": False, "agents": ["wellness"], "root": True}]
        listed, failed = tool(bridge, "drive_list", folder="onedrive:root")
        assert not failed and "Docs" in listed and "Partagé" not in listed, listed
        read, failed = tool(bridge, "drive_read", file="onedrive:X")
        assert not failed and "secret" in read, "the whole OneDrive: Privé's file too"
    finally:
        service.close()


def test_toujours_on_a_file_card_opens_nothing(bridges, fakes):
    """Review 2: « always » answered to a file's card (calendar-always' « Toujours autoriser ») is a plain « Oui »:
    the file is saved, no « Toujours » is kept, and the next new file asks again."""
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        connect(bridge, service, "dropbox")
        folders_put(bridge, "dropbox", [{"id": "id:w", "agents": ["wellness"], "writers": ["wellness"]}])
        connect(bridge, service, "dropbox")
        result, card = write_answered(bridge, "always", folder="dropbox:id:w", name="Un.md", text="1")
        assert not result["isError"] and "bip_always" not in card, (result, card)
        assert service.dropbox_content[service.written[-1][1]] == b"1"
        assert bridge.get("/v1/approval-level", headers=AUTH).json()["always"] == {}, "no « Toujours » kept"
        result, card = write_answered(bridge, "deny", folder="dropbox:id:w", name="Deux.md", text="2")
        assert card["bip_ok"]["kind"] == "drive_write", "asked again"
        result, card = write_answered(bridge, "always", replace="dropbox:id:f", text="v2")
        assert card["bip_ok"]["kind"] == "drive_replace" and not result["isError"], result
        assert bridge.get("/v1/approval-level", headers=AUTH).json()["always"] == {}
    finally:
        service.close()


def test_a_bridge_put_back_still_reads_the_levels_after_a_file_made_at_once(bridges, fakes):
    """Review C3: the bridge before drive-write (`BASE_BRIDGE_CMD`, int-1.1c's release, `… serve`), started on the
    same data after a new file was made at once, still reads the person's level: what undoes the file is kept in
    drive-undo.json, never in approval-level.json."""
    base = os.environ.get("BASE_BRIDGE_CMD", "").strip()
    if not base:
        pytest.skip("BASE_BRIDGE_CMD: the bridge before drive-write, for the rollback")
    service = FakeDrives()
    try:
        bridge = install(bridges, fakes, service)
        connect(bridge, service, "dropbox")
        folders_put(bridge, "dropbox", [{"id": "id:w", "agents": ["wellness"], "writers": ["wellness"]}])
        connect(bridge, service, "dropbox")
        assert bridge.put("/v1/approval-level", headers=AUTH, json={"level": "critical"}).status_code == 200
        result = call(bridge, "drive_write", {"folder": "dropbox:id:w", "name": "Notes", "text": "à garder"})
        assert not result["isError"] and "Done at once" in text_of(result), result
        kept = (bridge.data / "approval-level.json").read_text()
        assert "drive_file" not in kept and (bridge.data / "drive-undo.json").exists()
        bridge.stop()
        bridge.command = shlex.split(base)
        bridge.start()
        level = bridge.get("/v1/approval-level", headers=AUTH)
        assert level.status_code == 200 and level.json()["level"] == "critical", level.text
        lines = bridge.get("/v1/approvals/auto", headers=AUTH, params={"agent": "wellness"}).json()["items"]
        assert [l["kind"] for l in lines] == ["drive_write"] and lines[0]["undo"] is False, lines
    finally:
        service.close()
