#include "st-traffic.h"

#ifdef ANDROID
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define SLOTS 2048u
#define ROW_SIZE 64u

typedef struct {
    uint8_t key[40]; /* protocol, family, padding, ports, local IP, remote IP */
    uint64_t up;
    uint64_t down;
    uint8_t syn;
    uint8_t padding[7];
} TrafficRow;

_Static_assert(sizeof(TrafficRow) == ROW_SIZE, "unexpected traffic row layout");
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static atomic_bool enabled;
static TrafficRow rows[SLOTS + 1]; /* final slot contains unattributed bytes */

static int flow_key(const uint8_t *ip, size_t length, int down, uint8_t key[40])
{
    if (length < 20)
        return 0;
    unsigned int version = ip[0] >> 4;
    unsigned int offset;
    unsigned int protocol;
    const uint8_t *source, *target;
    size_t address_size;
    if (version == 4) {
        offset = (ip[0] & 15u) * 4;
        if (offset < 20 || (ip[6] & 31u) || ip[7])
            return 0;
        protocol = ip[9];
        source = ip + 12;
        target = ip + 16;
        address_size = 4;
    } else if (version == 6 && length >= 40) {
        offset = 40;
        protocol = ip[6];
        source = ip + 8;
        target = ip + 24;
        address_size = 16;
        while (offset + 8 <= length) {
            unsigned int extra;
            if (protocol == 0 || protocol == 43 || protocol == 60) {
                extra = (ip[offset + 1] + 1u) * 8;
            } else if (protocol == 44) {
                if (ip[offset + 2] || (ip[offset + 3] & 248u))
                    return 0;
                extra = 8;
            } else {
                break;
            }
            protocol = ip[offset];
            offset += extra;
        }
    } else {
        return 0;
    }
    if ((protocol != 6 && protocol != 17) || offset + 4 > length)
        return 0;

    key[0] = protocol;
    key[1] = version;
    memcpy(key + 4, ip + offset + (down ? 2 : 0), 2);
    memcpy(key + 6, ip + offset + (down ? 0 : 2), 2);
    memcpy(key + 8, down ? target : source, address_size);
    memcpy(key + 24, down ? source : target, address_size);
    return !down && protocol == 6 && offset + 14 <= length &&
           (ip[offset + 13] & 0x12u) == 0x02u;
}

void st_traffic_packet(struct pbuf *packet, unsigned int size, int down)
{
    if (!atomic_load_explicit(&enabled, memory_order_relaxed))
        return;
    uint8_t header[160];
    size_t length = packet->tot_len < sizeof(header) ? packet->tot_len : sizeof(header);
    const uint8_t *ip = packet->payload;
    if (packet->len < length) {
        length = pbuf_copy_partial(packet, header, length, 0);
        ip = header;
    }
    uint8_t key[40] = {0};
    int syn = flow_key(ip, length, down, key);

    pthread_mutex_lock(&lock);
    TrafficRow *row = &rows[SLOTS];
    if (key[0]) {
        uint32_t hash = 2166136261u;
        for (size_t i = 0; i < sizeof(key); ++i)
            hash = (hash ^ key[i]) * 16777619u;
        for (unsigned int i = 0; i < 12; ++i) {
            TrafficRow *slot = &rows[(hash + i) & (SLOTS - 1)];
            if (!slot->key[0] || !memcmp(slot->key, key, sizeof(key))) {
                memcpy(slot->key, key, sizeof(key));
                row = slot;
                break;
            }
        }
    }
    if (down)
        row->down += size;
    else
        row->up += size;
    row->syn |= syn;
    pthread_mutex_unlock(&lock);
}

void st_traffic_enable(JNIEnv *env, jobject service, jboolean value)
{
    pthread_mutex_lock(&lock);
    memset(rows, 0, sizeof(rows));
    atomic_store_explicit(&enabled, value != JNI_FALSE, memory_order_relaxed);
    pthread_mutex_unlock(&lock);
}

jbyteArray st_traffic_drain(JNIEnv *env, jobject service)
{
    TrafficRow *result = malloc(sizeof(rows));
    if (!result)
        return NULL;
    size_t count = 0;
    pthread_mutex_lock(&lock);
    for (size_t i = 0; i <= SLOTS; ++i) {
        if (rows[i].up || rows[i].down) {
            result[count++] = rows[i];
            memset(&rows[i], 0, sizeof(TrafficRow));
        }
    }
    pthread_mutex_unlock(&lock);

    jbyteArray bytes = (*env)->NewByteArray(env, count * ROW_SIZE);
    if (bytes && count)
        (*env)->SetByteArrayRegion(env, bytes, 0, count * ROW_SIZE, (const jbyte *)result);
    free(result);
    return bytes;
}
#endif
