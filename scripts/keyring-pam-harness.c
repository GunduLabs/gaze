// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Synthetic credentials only. Build as both a mock module and a Linux-PAM driver.
#include <security/pam_appl.h>
#include <security/pam_modules.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef GAZE_MOCK_MODULE
PAM_EXTERN int pam_sm_authenticate(pam_handle_t *pamh, int flags, int argc, const char **argv) {
    (void)flags;
    if (argc != 3) return PAM_SERVICE_ERR;
    if (strcmp(argv[0], "gaze") == 0) {
        int result = atoi(argv[1]);
        if (result == PAM_SUCCESS && strcmp(argv[2], "token") == 0)
            return pam_set_item(pamh, PAM_AUTHTOK, "synthetic-test-password");
        if (result == PAM_SUCCESS && strcmp(argv[2], "empty") == 0)
            return pam_set_item(pamh, PAM_AUTHTOK, "");
        return result;
    }
    const void *token = NULL;
    if (pam_get_item(pamh, PAM_AUTHTOK, &token) != PAM_SUCCESS) return PAM_SERVICE_ERR;
    int valid = strcmp(argv[2], "token") == 0
        ? token && strcmp(token, "synthetic-test-password") == 0
        : strcmp(argv[2], "empty") == 0 ? token && !*(const char *)token : token == NULL;
    FILE *marker = fopen(argv[1], "w");
    if (!marker) return PAM_SERVICE_ERR;
    fputs(valid ? "valid\n" : "invalid\n", marker);
    fclose(marker);
    if (strcmp(argv[0], "kwallet") == 0 && valid) {
        // KWallet caches auth data until open_session and returns PAM_IGNORE even on success.
        pam_set_data(pamh, "mock-wallet", (void *)"cached", NULL);
        return PAM_IGNORE;
    }
    return valid ? PAM_SUCCESS : PAM_AUTH_ERR;
}

PAM_EXTERN int pam_sm_open_session(pam_handle_t *pamh, int flags, int argc, const char **argv) {
    (void)flags;
    if (argc != 2) return PAM_SERVICE_ERR;
    const void *cached = NULL;
    if (pam_get_data(pamh, "mock-wallet", &cached) != PAM_SUCCESS || !cached)
        return PAM_SESSION_ERR;
    FILE *marker = fopen(argv[1], "w");
    if (!marker) return PAM_SERVICE_ERR;
    fputs("valid-session\n", marker);
    fclose(marker);
    return PAM_SUCCESS;
}
#else
static int no_prompt(int count, const struct pam_message **messages,
                     struct pam_response **responses, void *data) {
    (void)count; (void)messages; (void)responses; (void)data;
    return PAM_CONV_ERR;
}

int main(int argc, char **argv) {
    if (argc != 3 && argc != 4) return 2;
    struct pam_conv conversation = { no_prompt, NULL };
    pam_handle_t *pamh = NULL;
    int result = pam_start_confdir(argc == 4 ? argv[3] : "gdm-face", "synthetic-user", &conversation, argv[1], &pamh);
    if (result != PAM_SUCCESS) return 2;
    result = pam_authenticate(pamh, 0);
    if (argc == 4 && result == PAM_SUCCESS)
        result = pam_open_session(pamh, 0);
    pam_end(pamh, result);
    return ((result == PAM_SUCCESS) == (strcmp(argv[2], "success") == 0)) ? 0 : 1;
}
#endif
