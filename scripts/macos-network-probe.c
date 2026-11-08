// Raw UDP probe for per-app interception: deliberately has no SOCKS client.
// Build and ad-hoc sign it inside a .app with its own bundle identifier.
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

static int udp_check(void) {
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) { perror("socket"); return 1; }
    struct timeval timeout = {.tv_sec = 8};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    struct sockaddr_in target = {.sin_family = AF_INET, .sin_port = htons(53)};
    inet_pton(AF_INET, "1.1.1.1", &target.sin_addr);
    unsigned char query[] = {0,0,1,0,0,1,0,0,0,0,0,0,
        7,'e','x','a','m','p','l','e',3,'c','o','m',0,0,1,0,1};
    for (int i = 0; i < 8; ++i) {
        unsigned short transaction = (unsigned short)arc4random();
        query[0] = transaction >> 8; query[1] = transaction & 0xff;
        if (sendto(fd, query, sizeof(query), 0, (struct sockaddr *)&target, sizeof(target)) != sizeof(query)) {
            perror("sendto"); close(fd); return 1;
        }
        unsigned char reply[4096];
        ssize_t count = recvfrom(fd, reply, sizeof(reply), 0, NULL, NULL);
        if (count < 12) { perror("recvfrom"); close(fd); return 1; }
        if (reply[0] != query[0] || reply[1] != query[1] || !(reply[2] & 0x80) || (reply[3] & 0xf)) {
            fprintf(stderr, "Invalid DNS response\n"); close(fd); return 1;
        }
    }
    close(fd);
    puts("PASS: 8 raw UDP DNS responses (application has no proxy settings)");
    return 0;
}

static int tcp_check(void) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) { perror("socket"); return 1; }
    struct timeval timeout = {.tv_sec = 8};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    struct sockaddr_in target = {.sin_family = AF_INET, .sin_port = htons(80)};
    inet_pton(AF_INET, "1.1.1.1", &target.sin_addr);
    if (connect(fd, (struct sockaddr *)&target, sizeof(target)) != 0) {
        perror("connect"); close(fd); return 1;
    }
    const char request[] = "HEAD / HTTP/1.1\r\nHost: one.one.one.one\r\nConnection: close\r\n\r\n";
    if (send(fd, request, sizeof(request)-1, 0) != sizeof(request)-1) {
        perror("send"); close(fd); return 1;
    }
    char reply[4096]; ssize_t count = recv(fd, reply, sizeof(reply), 0);
    close(fd);
    if (count < 5 || memcmp(reply, "HTTP/", 5)) { fprintf(stderr, "No HTTP response\n"); return 1; }
    puts("PASS: raw TCP HTTP response (application has no proxy settings)");
    return 0;
}

int main(int argc, char **argv) {
    return argc > 1 && !strcmp(argv[1], "tcp") ? tcp_check() : udp_check();
}
