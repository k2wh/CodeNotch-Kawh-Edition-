// A console window flashing up behind a HUD overlay would rather defeat the
// point, so release builds attach to the Windows subsystem instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    codenotch_lib::run()
}
