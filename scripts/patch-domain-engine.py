"""Patch a disposable copy of the pinned ProxyBridge sources, never the checkout."""
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
p = root / 'pb_rules.c'
s = p.read_text(encoding='utf-8-sig')
s = 'extern int st_regex_match(const char *, const char *);\n' + s
needle = 'BOOL match_domain_pattern(const char *pattern, const char *domain)\n{'
assert s.count(needle) == 1
s = s.replace(needle, needle + '\n    if (pattern && strncmp(pattern, "stregex:", 8) == 0)\n        return domain && domain[0] && st_regex_match(pattern, domain);')
# Fail closed at profile load, rather than silently ignoring malformed regex.
needle = '    // target_domains: "" or NULL means no domain restriction (stored as "*")'
assert s.count(needle) == 1
s = s.replace(needle, '''    if (target_domains && strncmp(target_domains, "stregex:", 8) == 0 && !st_regex_match(target_domains, "")) {
        free(rule->target_hosts); free(rule->target_ports); free(rule); return 0;
    }
''' + needle)
# Disabled rules remain observable, although they never select a route.
needle = '        if (rule->enabled)\n        {\n            has_active = TRUE;'
assert s.count(needle) == 1
s = s.replace(needle, '''        if (rule->target_domains && strncmp(rule->target_domains, "stregex:", 8) == 0)
            has_domain = TRUE;
''' + needle)
p.write_text(s, encoding='utf-8')

p = root / 'ProxyBridge.c'
s = p.read_text(encoding='utf-8-sig')
# A routing event uses the same deduplication as the existing connection log.
# This is a decision counter, not a packet/delivery or HTTP-request counter.
entries = [
    ('pid6u, v6key, dp, sp, TRUE, action6u', '0', '(const UINT8*)ipv6_header->DstAddr', 'action6u'),
    ('pid6, v6key, dp, sp, FALSE, action6', '0', '(const UINT8*)ipv6_header->DstAddr', 'action6'),
    ('pid, dest_ip, dest_port, src_port, TRUE, action', 'dest_ip', 'NULL', 'action'),
    ('pid, orig_dest_ip, orig_dest_port, src_port, FALSE, action', 'orig_dest_ip', 'NULL', 'action'),
]
for args, ip, ip6, action in entries:
    pattern = r'(if \(connection_log_wanted\(' + re.escape(args) + r'\)\)\s*\{)'
    s, count = re.subn(pattern, lambda m: m[1] + f'\n st_domain_event({ip}, {ip6}, {action});', s)
    assert count == 1, args
helper = '''
static void st_domain_event(UINT32 ip, const UINT8 *ip6, RuleAction action) {
    if (!g_has_domain_rules) return;
    char domain[256] = {0};
    if (ip6) dns_cache_lookup_v6(ip6, domain, sizeof(domain));
    else dns_cache_lookup(ip, domain, sizeof(domain));
    char hex[513]; static const char digits[] = "0123456789abcdef";
    size_t i; for (i=0; domain[i] && i<255; ++i) {
        unsigned char c=(unsigned char)domain[i]; hex[i*2]=digits[c>>4]; hex[i*2+1]=digits[c&15];
    }
    hex[i*2]=0;
    log_message("[ST_ROUTE] %u %s %s", g_tl_rule_id,
        action == RULE_ACTION_PROXY ? "tunnel" : "direct", i ? hex : "-");
}
'''
needle = '#include "pb_internal.h"'
assert s.count(needle) == 1
s = s.replace(needle, needle + '\n' + helper)
p.write_text(s, encoding='utf-8')
