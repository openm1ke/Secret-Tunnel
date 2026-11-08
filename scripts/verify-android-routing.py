#!/usr/bin/env python3
"""Check captured emulator evidence without putting server/IP secrets in a report."""
import json
from pathlib import Path

root = Path(__file__).resolve().parent.parent
evidence = root / "artifacts/android/qa"


def read(name):
    return json.loads((evidence / name).read_text())


baseline = read("baseline.private.json")
selected = read("selected.private.json")
direct = read("direct.private.json")
oracle = read("server-egress.private.json")
running = read("integration-status.json")
stopped = read("integration-stop.json")
after = read("after-stop.private.json")
checks = {
    "selected_uid_uses_vpn": selected["vpn"] is True,
    "selected_https_200": selected.get("https_status") == 200,
    "selected_system_dns": selected.get("dns") is True,
    "selected_udp_dns_8_of_8": selected.get("udp_dns_replies") == 8,
    "selected_egress_matches_trusted_ssh_server": selected.get("egress_ip") == oracle["egress_ip"],
    "unselected_uid_has_no_vpn": direct["vpn"] is False,
    "unselected_egress_matches_baseline": direct.get("egress_ip") == baseline["egress_ip"],
    "ssh_rx_increased": running["native"]["rx"] > running["before"]["rx"],
    "ssh_tx_increased": running["native"]["tx"] > running["before"]["tx"],
    "stop_closes_vpn_and_ssh": stopped["active"] is False and stopped["native"]["running"] is False,
    "selected_app_goes_direct_after_manual_stop": after["vpn"] is False and after.get("passed") is True,
}
report = {
    "passed": all(checks.values()), "checks": checks,
    "rx_bytes": running["native"]["rx"], "tx_bytes": running["native"]["tx"],
    "same_external_ip_for_host_and_ssh": direct.get("egress_ip") == selected.get("egress_ip"),
    "note": "IP equality alone does not prove routing; UID VPN capabilities and increasing SSH payload counters were also checked.",
}
(evidence / "result.json").write_text(json.dumps(report, indent=2) + "\n")
for check, passed in checks.items():
    print(("PASS" if passed else "FAIL") + ": " + check)
raise SystemExit(0 if report["passed"] else 1)
