#include "utun.h"

#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/types.h>

/* <sys/kern_control.h> and <sys/sys_domain.h> are not in the iOS SDK; these
 * layouts are stable kernel ABI (same approach as WireGuard for iOS). */
#define ST_CTLIOCGINFO 0xc0644e03UL
#define ST_AF_SYSTEM 32

struct st_ctl_info {
    u_int32_t ctl_id;
    char ctl_name[96];
};

struct st_sockaddr_ctl {
    u_char sc_len;
    u_char sc_family;
    u_int16_t ss_sysaddr;
    u_int32_t sc_id;
    u_int32_t sc_unit;
    u_int32_t sc_reserved[5];
};

int st_find_utun_fd(void) {
    struct st_ctl_info info;
    memset(&info, 0, sizeof(info));
    strncpy(info.ctl_name, "com.apple.net.utun_control", sizeof(info.ctl_name) - 1);

    for (int fd = 0; fd <= 1024; fd++) {
        struct st_sockaddr_ctl addr;
        socklen_t len = sizeof(addr);
        memset(&addr, 0, sizeof(addr));
        if (getpeername(fd, (struct sockaddr *)&addr, &len) != 0 || addr.sc_family != ST_AF_SYSTEM) {
            continue;
        }
        if (info.ctl_id == 0 && ioctl(fd, ST_CTLIOCGINFO, &info) != 0) {
            continue;
        }
        if (addr.sc_id == info.ctl_id) {
            return fd;
        }
    }
    return -1;
}
