#!/bin/sh
set -e
pam-auth-update --package --remove gaze
pam-auth-update --package --remove gaze-simultaneous
