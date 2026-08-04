#!/usr/bin/env python3
"""Apply the Geo Relay overlay to a clean rustdesk/rustdesk-server checkout."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    content = path.read_text(encoding="utf-8")
    count = content.count(old)
    if count != 1:
        raise RuntimeError(f"expected one patch anchor in {path}, found {count}: {old!r}")
    path.write_text(content.replace(old, new, 1), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("upstream", type=Path, help="clean rustdesk-server checkout")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parents[1]
    upstream = args.upstream.resolve()
    if not (upstream / "Cargo.toml").is_file() or not (upstream / "src/rendezvous_server.rs").is_file():
        raise SystemExit(f"not a rustdesk-server checkout: {upstream}")

    shutil.copyfile(repo_root / "overlay/src/geo_relay.rs", upstream / "src/geo_relay.rs")
    shutil.copytree(
        repo_root / "overlay/src/geo_relay",
        upstream / "src/geo_relay",
        dirs_exist_ok=True,
    )

    cargo = upstream / "Cargo.toml"
    cargo_text = cargo.read_text(encoding="utf-8")
    if 'maxminddb = { version = "0.30", features = ["mmap"] }' not in cargo_text:
        replace_once(
            cargo,
            'flate2 = "1.0"\n',
            'flate2 = "1.0"\nmaxminddb = { version = "0.30", features = ["mmap"] }\n',
        )

    cargo_text = cargo.read_text(encoding="utf-8")
    if 'serde_yml = "0.0.13"' not in cargo_text:
        replace_once(
            cargo,
            'maxminddb = { version = "0.30", features = ["mmap"] }\n',
            'maxminddb = { version = "0.30", features = ["mmap"] }\n'
            'serde_yml = "0.0.13"\n',
        )

    lib = upstream / "src/lib.rs"
    lib_text = lib.read_text(encoding="utf-8")
    if "mod geo_relay;" not in lib_text:
        replace_once(lib, "mod rendezvous_server;\n", "mod rendezvous_server;\nmod geo_relay;\n")

    rendezvous = upstream / "src/rendezvous_server.rs"
    rendezvous_text = rendezvous.read_text(encoding="utf-8")
    if "use crate::geo_relay;" not in rendezvous_text:
        replace_once(
            rendezvous,
            "use crate::common::*;\nuse crate::peer::*;\n",
            "use crate::common::*;\nuse crate::geo_relay;\nuse crate::peer::*;\n",
        )

    rendezvous_text = rendezvous.read_text(encoding="utf-8")
    if "Geo relay startup:" not in rendezvous_text:
        replace_once(
            rendezvous,
            '        rs.parse_relay_servers(&get_arg("relay-servers"));\n',
            '        rs.parse_relay_servers(&get_arg("relay-servers"));\n'
            '        log::info!("Geo relay startup: {}", geo_relay::reload());\n',
        )

    rendezvous_text = rendezvous.read_text(encoding="utf-8")
    if "geo_relay::select_relay" not in rendezvous_text:
        old = '''    fn get_relay_server(&self, _pa: IpAddr, _pb: IpAddr) -> String {
        if self.relay_servers.is_empty() {
            return "".to_owned();
        } else if self.relay_servers.len() == 1 {
            return self.relay_servers[0].clone();
        }
        let i = ROTATION_RELAY_SERVER.fetch_add(1, Ordering::SeqCst) % self.relay_servers.len();
        self.relay_servers[i].clone()
    }
'''
        new = '''    fn get_relay_server(&self, pa: IpAddr, pb: IpAddr) -> String {
        if self.relay_servers.is_empty() {
            return "".to_owned();
        }
        if let Some(relay) = geo_relay::select_relay(pa, pb, self.relay_servers.as_ref()) {
            return relay;
        }
        if self.relay_servers.len() == 1 {
            return self.relay_servers[0].clone();
        }
        let i = ROTATION_RELAY_SERVER.fetch_add(1, Ordering::SeqCst) % self.relay_servers.len();
        self.relay_servers[i].clone()
    }
'''
        replace_once(rendezvous, old, new)

    rendezvous_text = rendezvous.read_text(encoding="utf-8")
    if 'Some("reload-geo" | "rg") =>' not in rendezvous_text:
        replace_once(
            rendezvous,
            '            Some("test-geo" | "tg") => {\n',
            '            Some("reload-geo" | "rg") => {\n'
            '                res = geo_relay::reload();\n'
            '            }\n'
            '            Some("test-geo" | "tg") => {\n',
        )

    print(f"Geo Relay overlay applied to {upstream}")


if __name__ == "__main__":
    main()
