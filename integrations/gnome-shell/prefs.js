// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences, gettext as _} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const GAZE_DBUS_NAME = 'com.gundulabs.Gaze';
const GAZE_OBJECT_PATH = '/com/gundulabs/Gaze';
const PROMPT_TIMEOUT_MS = 120000;

const MAX_TRIES_KEY = 'max-face-tries';
const FACE_AUTH_KEY = 'enable-face-authentication';
const RETRY_MODE_KEY = 'face-retry-mode';

function callGaze(method, params) {
    const conn = Gio.DBus.system;
    return new Promise((resolve, reject) => {
        conn.call(
            GAZE_DBUS_NAME, GAZE_OBJECT_PATH, GAZE_DBUS_NAME, method, params,
            null, Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, PROMPT_TIMEOUT_MS, null,
            (_src, res) => {
                try {
                    resolve(conn.call_finish(res));
                } catch (e) {
                    reject(e);
                }
            }
        );
    });
}
export default class GazePreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const extensionSettings = this.getSettings();

        const behaviorPage = new Adw.PreferencesPage({
            title: _('Behavior'),
            icon_name: 'preferences-system-symbolic',
        });

        const behaviorGroup = new Adw.PreferencesGroup({
            title: _('Face authentication'),
            description:
                _('Face authentication for unlocking this session. This does not affect the GDM login screen (see below). Settings are stored in your current dconf profile.'),
        });

        const faceRow = new Adw.SwitchRow({
            title: _('Enable face authentication (lock screen)'),
            active: extensionSettings.get_boolean(FACE_AUTH_KEY),
        });

        faceRow.connect('notify::active', row => {
            extensionSettings.set_boolean(FACE_AUTH_KEY, row.active);
        });
        extensionSettings.connect(`changed::${FACE_AUTH_KEY}`, () => {
            faceRow.set_active(extensionSettings.get_boolean(FACE_AUTH_KEY));
        });
        behaviorGroup.add(faceRow);

        const retryModes = ['disabled', 'fixed', 'infinite'];
        const retryModeRow = new Adw.ComboRow({
            title: _('Face retry mode'),
            model: Gtk.StringList.new([
                _('Disabled'),
                _('Fixed tries'),
                _('Infinite')
            ]),
        });
        behaviorGroup.add(retryModeRow);

        const triesRow = new Adw.SpinRow({
            title: _('Maximum face tries'),
            adjustment: new Gtk.Adjustment({
                lower: 2,
                upper: 20,
                step_increment: 1,
                page_increment: 1,
                value: extensionSettings.get_int(MAX_TRIES_KEY),
            }),
        });
        extensionSettings.bind(
            MAX_TRIES_KEY,
            triesRow,
            'value',
            Gio.SettingsBindFlags.DEFAULT
        );
        behaviorGroup.add(triesRow);

        const updateTriesRowSensitivity = (mode) => {
            triesRow.sensitive = (mode === 'fixed');
        };

        const currentMode = extensionSettings.get_string(RETRY_MODE_KEY);
        const initialIndex = retryModes.indexOf(currentMode);
        if (initialIndex !== -1) {
            retryModeRow.selected = initialIndex;
        }
        updateTriesRowSensitivity(currentMode);

        retryModeRow.connect('notify::selected', () => {
            const selectedMode = retryModes[retryModeRow.selected];
            if (selectedMode) {
                extensionSettings.set_string(RETRY_MODE_KEY, selectedMode);
                updateTriesRowSensitivity(selectedMode);
            }
        });

        extensionSettings.connect(`changed::${RETRY_MODE_KEY}`, () => {
            const val = extensionSettings.get_string(RETRY_MODE_KEY);
            const idx = retryModes.indexOf(val);
            if (idx !== -1 && retryModeRow.selected !== idx) {
                retryModeRow.selected = idx;
            }
            updateTriesRowSensitivity(val);
        });

        const facesGroup = new Adw.PreferencesGroup({title: _('Enrollment')});
        const facesRow = new Adw.ActionRow({
            title: _('Enrolled faces'),
            subtitle: _('Checking…'),
        });
        const openButton = new Gtk.Button({
            label: _('Open Gaze'),
            valign: Gtk.Align.CENTER,
        });
        openButton.connect('clicked', () => {
            try {
                Gio.AppInfo.create_from_commandline(
                    'gaze-gui', 'Gaze', Gio.AppInfoCreateFlags.NONE
                ).launch([], null);
            } catch (e) {
                logError(e, '[gaze] Failed to open the Gaze app');
                facesRow.set_subtitle(_('Could not open the Gaze app. Run gaze-gui from a terminal.'));
            }
        });
        facesRow.add_suffix(openButton);
        facesGroup.add(facesRow);

        callGaze('HasEnrolledFaces', new GLib.Variant('(s)', [GLib.get_user_name()]))
            .then(result => {
                const [enrolled] = result.deepUnpack();
                facesRow.set_subtitle(enrolled
                    ? _('A face is enrolled for this account.')
                    : _('No face is enrolled yet, so face unlock has nothing to match. Open Gaze to enroll one.'));
            })
            .catch(error => {
                logError(error, '[gaze] Failed to read enrolled faces');
                facesRow.set_subtitle(_('Gaze daemon unavailable.'));
            });

        behaviorPage.add(facesGroup);
        behaviorPage.add(behaviorGroup);

        const loginGroup = new Adw.PreferencesGroup({
            title: _('GDM login screen'),
            description:
                _('Enable face authentication at the GDM login screen. Requires administrator authorization. Note: GNOME keyring is normally unlocked by your password, so logging in with face only may leave it locked.'),
        });

        const gdmRow = new Adw.SwitchRow({
            title: _('Enable face authentication at GDM login'),
            active: false,
            sensitive: false,
        });

        let suppressGdmNotify = false;
        const setGdmRow = active => {
            suppressGdmNotify = true;
            gdmRow.set_active(active);
            suppressGdmNotify = false;
        };

        const loadGdmState = () => {
            callGaze('GetGdmFaceAuth', null)
                .then(result => {
                    const [enabled] = result.deepUnpack();
                    setGdmRow(enabled);
                    gdmRow.set_subtitle('');
                    gdmRow.set_sensitive(true);
                })
                .catch(error => {
                    logError(error, '[gaze] Failed to read GDM face auth state');
                    gdmRow.set_subtitle(_('Gaze daemon unavailable.'));
                });
        };

        const daemonWatch = Gio.bus_watch_name(
            Gio.BusType.SYSTEM, GAZE_DBUS_NAME, Gio.BusNameWatcherFlags.NONE,
            loadGdmState,
            () => {
                gdmRow.set_sensitive(false);
                gdmRow.set_subtitle(_('Gaze daemon unavailable.'));
            }
        );
        window.connect('close-request', () => {
            Gio.bus_unwatch_name(daemonWatch);
            return false;
        });

        const notifyGdmFailure = (error, desired) => {
            const accessDenied =
                Gio.DBusError.is_remote_error(error) &&
                Gio.DBusError.get_remote_error(error) ===
                    'org.freedesktop.DBus.Error.AccessDenied';
            let message;
            if (accessDenied) {
                message = desired
                    ? _('Administrator authorization is required to enable face authentication at the GDM login screen.')
                    : _('Administrator authorization is required to disable face authentication at the GDM login screen.');
            } else {
                Gio.DBusError.strip_remote_error(error);
                message = _('Could not update GDM login face authentication: {error}')
                    .replace('{error}', () => error.message);
            }
            gdmRow.set_subtitle(message);
            if (typeof window.add_toast === 'function')
                window.add_toast(new Adw.Toast({title: message}));
        };

        let gdmRequestInFlight = false;
        gdmRow.connect('notify::active', row => {
            if (suppressGdmNotify || gdmRequestInFlight)
                return;
            const desired = row.active;
            gdmRequestInFlight = true;
            row.set_sensitive(false);
            callGaze('SetGdmFaceAuth', new GLib.Variant('(b)', [desired]))
                .then(() => {
                    gdmRequestInFlight = false;
                    gdmRow.set_subtitle('');
                    gdmRow.set_sensitive(true);
                    if (typeof window.add_toast === 'function') {
                        const message = desired
                            ? _('Face auth enabled at the GDM login screen. Restart GDM (or reboot) for it to take effect.')
                            : _('Face auth disabled at the GDM login screen. Restart GDM (or reboot) for it to take effect.');
                        window.add_toast(new Adw.Toast({title: message}));
                    }
                })
                .catch(error => {
                    logError(error, '[gaze] Failed to update GDM face auth');
                    gdmRequestInFlight = false;
                    setGdmRow(!desired);
                    gdmRow.set_sensitive(true);
                    notifyGdmFailure(error, desired);
                });
        });
        loginGroup.add(gdmRow);

        behaviorPage.add(loginGroup);

        window.add(behaviorPage);
    }
}
