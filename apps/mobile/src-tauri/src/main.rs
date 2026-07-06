// Desktop entry point; on mobile the library's `mobile_entry_point` is used.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    freedback_app_lib::run()
}
