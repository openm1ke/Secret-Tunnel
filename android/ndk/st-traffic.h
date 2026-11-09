#pragma once
#ifdef ANDROID
#include <jni.h>
#include <lwip/pbuf.h>

void st_traffic_packet(struct pbuf *packet, unsigned int size, int down);
void st_traffic_enable(JNIEnv *env, jobject service, jboolean enabled);
jbyteArray st_traffic_drain(JNIEnv *env, jobject service);
#endif
