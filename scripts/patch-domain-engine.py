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
needle = '__declspec(thread) UINT32 g_tl_rule_id = 0;'
assert s.count(needle) == 1
s = s.replace(needle, needle + '''
__declspec(thread) char st_webview_owner[MAX_PROCESS_NAME];
BOOL st_find_webview_owner(DWORD pid, const char *image, char *owner, DWORD capacity);

static BOOL st_match_process(const char *list, const char *image) {
    return match_process_list(list, image)
        || (st_webview_owner[0] && match_process_list(list, st_webview_owner));
}
''')
needle = 'if (match_process_list(rule->process_name, process_name))'
assert s.count(needle) == 2
s = s.replace(needle, 'if (st_match_process(rule->process_name, process_name))')
for suffix in ('v6', ''):
    needle = f'    RuleAction action = match_rule{"_" + suffix if suffix else ""}(process_name,'
    assert s.count(needle) == 1
    before = '''    st_webview_owner[0] = '\\0';
    if (g_proxy_config_count > 1)
        st_find_webview_owner(pid, process_name, st_webview_owner, sizeof(st_webview_owner));
'''
    pos = s.index(needle)
    end = s.index('\n', pos)
    s = s[:pos] + before + s[pos:end] + "\n    st_webview_owner[0] = '\\0';" + s[end:]
p.write_text(s, encoding='utf-8')

p = root / 'pb_process.c'
s = p.read_text(encoding='utf-8-sig')
s += r'''

static BOOL st_is_webview_image(const char *image) {
    const char *name = strrchr(image, '\\');
    return _stricmp(name ? name + 1 : image, "msedgewebview2.exe") == 0;
}

static BOOL st_creation_time(DWORD pid, FILETIME *created) {
    HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!process) return FALSE;
    FILETIME exited, kernel, user;
    BOOL ok = GetProcessTimes(process, created, &exited, &kernel, &user);
    CloseHandle(process);
    return ok;
}

BOOL st_find_webview_owner(DWORD pid, const char *image, char *owner, DWORD capacity) {
    if (!st_is_webview_image(image)) return FALSE;
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snapshot == INVALID_HANDLE_VALUE) return FALSE;

    BOOL found = FALSE;
    FILETIME child_created;
    if (st_creation_time(pid, &child_created)) {
        for (unsigned int depth = 0; depth < 8; ++depth) {
            PROCESSENTRY32W entry = {0};
            entry.dwSize = sizeof(entry);
            BOOL valid = Process32FirstW(snapshot, &entry);
            while (valid && entry.th32ProcessID != pid)
                valid = Process32NextW(snapshot, &entry);
            if (!valid || !entry.th32ParentProcessID || entry.th32ParentProcessID == pid)
                break;

            pid = entry.th32ParentProcessID;
            FILETIME parent_created;
            char parent[MAX_PROCESS_NAME];
            // A reused PID can otherwise bind a WebView to an unrelated app.
            if (!st_creation_time(pid, &parent_created)
                || CompareFileTime(&parent_created, &child_created) > 0
                || !get_process_name_from_pid(pid, parent, sizeof(parent)))
                break;
            child_created = parent_created;
            if (!st_is_webview_image(parent)) {
                found = strcpy_s(owner, capacity, parent) == 0;
                break;
            }
        }
    }
    CloseHandle(snapshot);
    return found;
}
'''
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
