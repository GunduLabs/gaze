import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const EXTENSION_SCHEMA_ID = 'org.gnome.shell.extensions.gaze';
const GAZE_BUS_NAME = 'com.gundulabs.Gaze';
const GAZE_OBJECT_PATH = '/com/gundulabs/Gaze';

const MAX_TRIES_KEY = 'max-face-tries';
const FACE_AUTH_KEY = 'enable-face-authentication';
const RETRY_MODE_KEY = 'face-retry-mode';

const GAZE_IFACE = 'com.gundulabs.Gaze';

function callGaze(method, params) {
    const conn = Gio.DBus.system;
    return new Promise((resolve, reject) => {
        conn.call(
            GAZE_BUS_NAME, GAZE_OBJECT_PATH, GAZE_IFACE, method, params,
            null, Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, -1, null,
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
        const extensionSettings = new Gio.Settings({schema_id: EXTENSION_SCHEMA_ID});

        const behaviorPage = new Adw.PreferencesPage({
            title: 'Behavior',
            icon_name: 'preferences-system-symbolic',
        });

        const behaviorGroup = new Adw.PreferencesGroup({
            title: 'Face authentication',
            description: 'Settings are stored in your current dconf profile.',
        });

        const faceRow = new Adw.SwitchRow({
            title: 'Enable face authentication',
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
            title: 'Face retry mode',
            model: Gtk.StringList.new([
                'Disabled',
                'Fixed tries',
                'Infinite'
            ]),
        });
        behaviorGroup.add(retryModeRow);

        const triesRow = new Adw.SpinRow({
            title: 'Maximum face tries',
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

        behaviorPage.add(behaviorGroup);

        const loginGroup = new Adw.PreferencesGroup({
            title: 'GDM login screen',
            description:
                'Enable face authentication at the GDM login screen. ' +
                'Requires administrator authorization. ' +
                'Note: GNOME keyring is normally unlocked by your password, ' +
                'so logging in with face only may leave it locked.',
        });

        const gdmRow = new Adw.SwitchRow({
            title: 'Enable face auth at GDM login',
            active: false,
            sensitive: false,
        });

        let suppressGdmNotify = false;
        const setGdmRow = active => {
            suppressGdmNotify = true;
            gdmRow.set_active(active);
            suppressGdmNotify = false;
        };

        callGaze('GetGdmFaceAuth', null)
            .then(result => {
                const [enabled] = result.deepUnpack();
                setGdmRow(enabled);
                gdmRow.set_sensitive(true);
            })
            .catch(error => {
                logError(error, '[gaze] Failed to read GDM face auth state');
                gdmRow.set_subtitle('Gaze daemon unavailable.');
            });

        const notifyGdmFailure = (error, desired) => {
            const accessDenied =
                Gio.DBusError.is_remote_error(error) &&
                Gio.DBusError.get_remote_error(error) ===
                    'org.freedesktop.DBus.Error.AccessDenied';
            let message;
            if (accessDenied) {
                message = desired
                    ? 'Administrator authorization is required to enable face auth at the GDM login screen.'
                    : 'Administrator authorization is required to disable face auth at the GDM login screen.';
            } else {
                Gio.DBusError.strip_remote_error(error);
                message = `Could not update GDM login face auth: ${error.message}`;
            }
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
                    gdmRow.set_sensitive(true);
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
