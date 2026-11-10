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
char *st_ios_check(const char *config_json);
void st_ios_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
