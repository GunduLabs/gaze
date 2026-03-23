import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const EXTENSION_SCHEMA_ID = 'org.gnome.shell.extensions.gaze';

const MAX_TRIES_KEY = 'max-face-tries';
const FACE_AUTH_KEY = 'enable-face-authentication';

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

        const triesRow = new Adw.SpinRow({
            title: 'Maximum face tries',
            adjustment: new Gtk.Adjustment({
                lower: 1,
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

        behaviorPage.add(behaviorGroup);

        window.add(behaviorPage);
    }
}
