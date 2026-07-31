// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

// Drives a PAM service the way KScreenLocker's greeter drives its biometric slot.

#include <security/pam_appl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int prompted = 0;

static int converse(int n, const struct pam_message **msg,
                    struct pam_response **resp, void *data) {
  (void)data;

  struct pam_response *replies = calloc((size_t)n, sizeof(struct pam_response));
  if (replies == NULL) {
    return PAM_BUF_ERR;
  }

  for (int i = 0; i < n; i++) {
    const char *text = msg[i]->msg ? msg[i]->msg : "";
    switch (msg[i]->msg_style) {
    case PAM_TEXT_INFO:
      printf("  [info    | not shown, sets hadPrompt] %s\n", text);
      break;
    case PAM_ERROR_MSG:
      printf("  [error   | shown on the lock screen ] %s\n", text);
      break;
    case PAM_PROMPT_ECHO_OFF:
    case PAM_PROMPT_ECHO_ON:
      printf("  [PROMPT  | FATAL                    ] %s\n", text);
      printf("        A real greeter blocks here forever: nothing routes a\n"
             "        response to a noninteractive authenticator.\n");
      prompted = 1;
      free(replies);
      return PAM_CONV_ERR;
    default:
      printf("  [style %d | unhandled                ] %s\n",
             msg[i]->msg_style, text);
      break;
    }
  }

  *resp = replies;
  return PAM_SUCCESS;
}

int main(int argc, char **argv) {
  const char *service = argc > 1 ? argv[1] : "kde-fingerprint";
  const char *user = argc > 2 ? argv[2] : getlogin();
  int rounds = argc > 3 ? atoi(argv[3]) : 1;

  if (user == NULL) {
    fprintf(stderr, "could not determine the current user; pass one\n");
    return 2;
  }

  printf("service=%s user=%s rounds=%d uid=%u\n", service, user, rounds,
         (unsigned)getuid());
  if (getuid() == 0) {
    printf("note: the real greeter runs unprivileged; rerun as your own user\n");
  }

  struct pam_conv conv = {converse, NULL};
  pam_handle_t *pamh = NULL;

  // One pam_start, many pam_authenticate, exactly as the greeter does.
  int rc = pam_start(service, user, &conv, &pamh);
  if (rc != PAM_SUCCESS) {
    fprintf(stderr, "pam_start: %s\n", pam_strerror(pamh, rc));
    return 1;
  }

  int failures = 0;
  int retired = 0;

  for (int round = 1; round <= rounds; round++) {
    printf("\n-- round %d --\n", round);
    prompted = 0;
    rc = pam_authenticate(pamh, 0);
    printf("  rc=%d (%s)\n", rc, pam_strerror(pamh, rc));

    if (prompted) {
      printf("  FAIL: the module prompted on %s\n", service);
      failures++;
    }

    if (rc == PAM_SUCCESS) {
      printf("  -> unlocks\n");
    } else if (rc == PAM_AUTHINFO_UNAVAIL || rc == PAM_MODULE_UNKNOWN) {
      printf("  -> slot retired for the rest of this lock (sticky), and any\n"
             "     fingerprint reader in the same stack goes with it\n");
      retired = 1;
    } else {
      printf("  -> attempt failed, slot stays armed for the next round\n");
    }

    if (retired && round < rounds) {
      printf("  (the greeter would not call pam_authenticate again)\n");
      break;
    }
  }

  pam_end(pamh, rc);

  printf("\n%s\n", failures == 0 ? "OK: no prompt was issued"
                                 : "FAILED: see the prompt warnings above");
  return failures == 0 ? 0 : 1;
}
