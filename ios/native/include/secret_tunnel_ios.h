#ifndef SECRET_TUNNEL_IOS_H
#define SECRET_TUNNEL_IOS_H

#ifdef __cplusplus
extern "C" {
#endif

/* Strings returned by these functions must be released with st_ios_free. */
char *st_ios_start(const char *config_json); /* "" on success, otherwise an error */
void st_ios_stop(void);
char *st_ios_status(void);
char *st_ios_probe(const char *config_json);
char *st_ios_generate_key(void);
char *st_ios_enroll_password(const char *config_json, const char *password);
char *st_ios_install_agent(const char *config_json, const char *version, const char *archive_path);
char *st_ios_check(const char *config_json);
char *st_ios_transfer_export(const char *document_json);
char *st_ios_transfer_unlock(const char *payload, const char *code, int preview);
void st_ios_free(char *value);
// Run on a worker thread. Returned string must be released with st_ios_free.
char *st_ios_hosting(const char *request, const char *cache_dir, int vpn_active);

#ifdef __cplusplus
}
#endif

#endif
