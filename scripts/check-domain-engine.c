// Uses actual native matching/DNS cache without starting WinDivert.
#include "pb_internal.h"
#include <assert.h>
extern int st_regex_match(const char *, const char *);
extern BOOL st_find_webview_owner(DWORD pid, const char *image, char *owner, DWORD capacity);
extern __declspec(thread) char st_webview_owner[MAX_PROCESS_NAME];
static void encode(const char *pattern, char *out) {
    strcpy(out,"stregex:"); out+=8;
    while(*pattern) {sprintf(out,"%02x",(unsigned char)*pattern++);out+=2;}
}
static void check_webview_owner(void) {
    char exe[MAX_PATH], directory[MAX_PATH], child[MAX_PATH], command[MAX_PATH + 32];
    assert(GetModuleFileNameA(NULL, exe, sizeof(exe)) > 0);
    assert(GetTempPathA(sizeof(directory), directory) > 0);
    char scratch[MAX_PATH];
    assert(GetTempFileNameA(directory, "stw", 0, scratch));
    assert(DeleteFileA(scratch));
    assert(CreateDirectoryA(scratch, NULL));
    assert(sprintf_s(child, sizeof(child), "%s\\msedgewebview2.exe", scratch) > 0);
    assert(CopyFileA(exe, child, TRUE));
    assert(sprintf_s(command, sizeof(command), "\"%s\" --webview-child", child) > 0);
    STARTUPINFOA startup = {0};
    PROCESS_INFORMATION process = {0};
    startup.cb = sizeof(startup);
    assert(CreateProcessA(child, command, NULL, NULL, FALSE, CREATE_SUSPENDED,
        NULL, NULL, &startup, &process));
    char owner[MAX_PROCESS_NAME] = {0};
    BOOL found = st_find_webview_owner(process.dwProcessId, child, owner, sizeof(owner));
    TerminateProcess(process.hProcess, 0);
    WaitForSingleObject(process.hProcess, 3000);
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    assert(DeleteFileA(child));
    assert(RemoveDirectoryA(scratch));
    assert(found && _stricmp(owner, exe) == 0);
}

int main(void) {
    dns_cache_init();
    UINT32 cfg=ProxyBridge_AddProxyConfig(PROXY_TYPE_SOCKS5,"127.0.0.1",1080,"","",TRUE);
    assert(cfg);
    char specific[2200]={0}, all[2200]={0}, comma[2200]={0};
    encode("(?:.*\\.)?example\\.com",specific);
    encode(".*",all); encode("[a-z]{1,4}\\.example\\.com",comma);
    assert(st_regex_match(comma,"cdn.example.com"));
    assert(!st_regex_match(specific,"example.com.evil"));
    assert(ProxyBridge_AddRule("client.exe","*","*","*",RULE_PROTOCOL_BOTH,RULE_ACTION_DIRECT,0,FALSE)==1);
    assert(ProxyBridge_AddRule("*","*","*",specific,RULE_PROTOCOL_BOTH,RULE_ACTION_DIRECT,0,FALSE)==2);
    assert(ProxyBridge_AddRule("*","*","*",all,RULE_PROTOCOL_BOTH,RULE_ACTION_PROXY,cfg,FALSE)==3);
    assert(ProxyBridge_DisableRule(3));
    assert(ProxyBridge_AddRule("firefox.exe","*","*","*",RULE_PROTOCOL_BOTH,RULE_ACTION_PROXY,cfg,FALSE)==4);
    assert(ProxyBridge_AddRule("*","*","*","*",RULE_PROTOCOL_BOTH,RULE_ACTION_DIRECT,0,FALSE)==5);
    UINT32 known=inet_addr("192.0.2.1"), unknown=inet_addr("192.0.2.2"), winner=0;
    dns_cache_store(known,"CDN.EXAMPLE.COM");
    assert(match_rule("firefox.exe",known,443,FALSE,&winner)==RULE_ACTION_DIRECT && g_tl_rule_id==2);
    assert(match_rule("firefox.exe",known,443,TRUE,&winner)==RULE_ACTION_DIRECT && g_tl_rule_id==2);
    assert(match_rule("firefox.exe",unknown,443,FALSE,&winner)==RULE_ACTION_PROXY && g_tl_rule_id==4);
    assert(match_rule("other.exe",unknown,443,FALSE,&winner)==RULE_ACTION_DIRECT && g_tl_rule_id==5);
    assert(match_rule("client.exe",known,443,FALSE,&winner)==RULE_ACTION_DIRECT && g_tl_rule_id==1);
    const UINT8 ip6[16]={0x20,1,0x0d,0xb8,0,0,0,0,0,0,0,0,0,0,0,1};
    dns_cache_store_v6(ip6,"example.com");
    assert(match_rule_v6("firefox.exe",ip6,443,TRUE,&winner)==RULE_ACTION_DIRECT && g_tl_rule_id==2);
    assert(ProxyBridge_DisableRule(2));
    assert(g_has_domain_rules); // disabled domain rules can still be observed
    assert(match_rule("firefox.exe",known,443,FALSE,&winner)==RULE_ACTION_PROXY && g_tl_rule_id==4);
    assert(ProxyBridge_EnableRule(3));
    assert(match_rule("other.exe",known,443,TRUE,&winner)==RULE_ACTION_PROXY && g_tl_rule_id==3);
    assert(!ProxyBridge_AddRule("*","*","*","stregex:5b",RULE_PROTOCOL_BOTH,RULE_ACTION_DIRECT,0,FALSE));
    assert(ProxyBridge_DisableRule(3));
    UINT32 secondary = ProxyBridge_AddProxyConfig(PROXY_TYPE_SOCKS5,"127.0.0.1",1081,"","",TRUE);
    assert(secondary && secondary != cfg);
    assert(ProxyBridge_AddRule("dorion.exe","*","*","*",RULE_PROTOCOL_UDP,RULE_ACTION_PROXY,secondary,FALSE));
    assert(ProxyBridge_AddRule("dorion.exe","*","*","*",RULE_PROTOCOL_BOTH,RULE_ACTION_PROXY,cfg,FALSE));
    strcpy_s(st_webview_owner, MAX_PROCESS_NAME, "C:\\Apps\\Dorion.exe");
    assert(match_rule("msedgewebview2.exe",unknown,443,TRUE,&winner)==RULE_ACTION_PROXY && winner==secondary);
    assert(match_rule("msedgewebview2.exe",unknown,443,FALSE,&winner)==RULE_ACTION_PROXY && winner==cfg);
    strcpy_s(st_webview_owner, MAX_PROCESS_NAME, "C:\\Apps\\Other.exe");
    assert(match_rule("msedgewebview2.exe",unknown,443,TRUE,&winner)==RULE_ACTION_DIRECT);
    st_webview_owner[0] = 0;
    check_webview_owner();
    puts("PASS: native IPv4/IPv6 TCP/UDP regex, direct override, disabled rules, priority, transport bypass, unknown DNS, invalid regex.");
    return 0;
}
