use adw::prelude::*;

pub(super) fn show_shortcuts_window(parent: &adw::ApplicationWindow) {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <object class="GtkShortcutsWindow" id="shortcuts_window">
    <property name="modal">true</property>
    <child>
      <object class="GtkShortcutsSection">
        <property name="title">General</property>
        <property name="section-name">general</property>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Application</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;n</property>
                <property name="title">Open New Window</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;comma</property>
                <property name="title">Open Preferences</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;question</property>
                <property name="title">Keyboard Shortcuts</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">F1</property>
                <property name="title">About Craic</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Repository</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;p</property>
                <property name="title">Pull remote changes</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;u</property>
                <property name="title">Push local commits</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;r</property>
                <property name="title">Refresh repository status</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">File Browser</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;c</property>
                <property name="title">Copy selected file entries</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;x</property>
                <property name="title">Cut selected file entries</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;v</property>
                <property name="title">Paste file entries</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">Delete</property>
                <property name="title">Delete selected file entry</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">Up</property>
                <property name="title">Select previous entry</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">Down</property>
                <property name="title">Select next entry</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">Return</property>
                <property name="title">Open / expand selected folder</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">Escape</property>
                <property name="title">Cancel file rename/new-entry mode</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Code Editor</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;f</property>
                <property name="title">Toggle search</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;a</property>
                <property name="title">Select all</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;z</property>
                <property name="title">Undo</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;z</property>
                <property name="title">Redo</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;y</property>
                <property name="title">Redo</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;c</property>
                <property name="title">Copy</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;Insert</property>
                <property name="title">Copy</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;x</property>
                <property name="title">Cut</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;v</property>
                <property name="title">Paste</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;w</property>
                <property name="title">Toggle word wrap</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;plus</property>
                <property name="title">Increase font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;equal</property>
                <property name="title">Increase font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Add</property>
                <property name="title">Increase font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;minus</property>
                <property name="title">Decrease font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;underscore</property>
                <property name="title">Decrease font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Subtract</property>
                <property name="title">Decrease font size</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Terminal</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;c</property>
                <property name="title">Copy terminal selection</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;C</property>
                <property name="title">Copy terminal selection</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;Insert</property>
                <property name="title">Copy terminal selection</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;v</property>
                <property name="title">Paste terminal selection</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Shift&gt;Insert</property>
                <property name="title">Paste terminal selection</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;BackSpace</property>
                <property name="title">Delete previous word</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;plus</property>
                <property name="title">Increase terminal font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;equal</property>
                <property name="title">Increase terminal font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Add</property>
                <property name="title">Increase terminal font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;minus</property>
                <property name="title">Decrease terminal font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;underscore</property>
                <property name="title">Decrease terminal font size</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Subtract</property>
                <property name="title">Decrease terminal font size</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">PDF Preview</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;plus</property>
                <property name="title">Zoom in</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;equal</property>
                <property name="title">Zoom in</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Add</property>
                <property name="title">Zoom in</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;minus</property>
                <property name="title">Zoom out</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;underscore</property>
                <property name="title">Zoom out</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;KP_Subtract</property>
                <property name="title">Zoom out</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="accelerator">&lt;Control&gt;c</property>
                <property name="title">Copy selected text</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </object>
</interface>
"#;

    let builder = gtk::Builder::from_string(xml);
    log::debug!("Displaying keyboard shortcuts window");
    if let Some(shortcuts_window) = builder.object::<gtk::ShortcutsWindow>("shortcuts_window") {
        shortcuts_window.set_transient_for(Some(parent));
        shortcuts_window.present();
    }
}
