// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stddef.h>
#include <stdint.h>

static int anchor;
static const void *get_api(uint32_t version) {
    (void)version;
    return &anchor;
}
static const char *get_version(void) { return "1.24.1"; }

struct ApiBase {
    const void *(*GetApi)(uint32_t);
    const char *(*GetVersionString)(void);
};
static const struct ApiBase api = { get_api, get_version };
const struct ApiBase *OrtGetApiBase(void) { return &api; }

const char *RuntimeOrigin(void) {
    Dl_info info;
    return dladdr(&anchor, &info) ? info.dli_fname : NULL;
}
