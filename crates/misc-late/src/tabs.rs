// Open terminal, down to the tab: a Windows Terminal window has one tab per
// shell, and an agent session runs in one of them. The tabs are read through UI
// Automation (their titles), the right one is chosen by `choose`, and selected
// with the pattern a click would use. No key is sent and nothing is typed.
//
// WT does not say which tab a process lives in, so the match is by title: the
// tab that names the session's folder, else the session's own title. A tab
// whose title says neither is left alone (`None`), never guessed.
// An elevated Terminal cannot be read from here; that is an error, not a miss.

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants, UIA_SelectionItemPatternId,
    UIA_TabItemControlTypeId,
};

#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// The session's working folder.
    pub cwd: String,
    /// Whatever else names it: the pill's name, the session title.
    pub title: String,
}

/// The last part of a path, either slash.
pub fn folder_name(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

fn has(tab: &str, word: &str) -> bool {
    word.len() >= 2 && tab.to_lowercase().contains(&word.to_lowercase())
}

/// The index of the tab that belongs to the session, or `None`. The whole path
/// in a title beats the folder name, which beats the session's title; the first
/// of equals wins.
pub fn choose(tabs: &[String], hints: &Hints) -> Option<usize> {
    let folder = folder_name(&hints.cwd);
    let mut best: Option<(u8, usize)> = None;
    for (i, tab) in tabs.iter().enumerate() {
        let score = if hints.cwd.len() > 3 && has(tab, hints.cwd.trim_end_matches(['\\', '/'])) {
            3
        } else if has(tab, &folder) {
            2
        } else if has(tab, &hints.title) {
            1
        } else {
            0
        };
        if score > 0 && best.is_none_or(|(s, _)| score > s) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Selects the tab of the session in the Terminal window `hwnd`. Returns its title.
pub fn select(hwnd: isize, hints: &Hints) -> Result<Option<String>, String> {
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let out = (|| -> windows::core::Result<Option<String>> {
        unsafe {
            let ui: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = ui.ElementFromHandle(HWND(hwnd as *mut _))?;
            let found = root.FindAll(TreeScope_Descendants, &ui.CreateTrueCondition()?)?;
            let mut items = Vec::new();
            for i in 0..found.Length()? {
                let el = found.GetElement(i)?;
                if el.CurrentControlType()? == UIA_TabItemControlTypeId {
                    items.push((el.CurrentName()?.to_string(), el));
                }
            }
            let titles: Vec<String> = items.iter().map(|(t, _)| t.clone()).collect();
            let Some(at) = choose(&titles, hints) else { return Ok(None) };
            let pattern: IUIAutomationSelectionItemPattern = items[at].1.GetCurrentPattern(UIA_SelectionItemPatternId)?.cast()?;
            pattern.Select()?;
            Ok(Some(titles[at].clone()))
        }
    })();
    if init.is_ok() {
        unsafe { CoUninitialize() };
    }
    out.map_err(|e| format!("Could not reach the Terminal's tabs: {e}"))
}

/// The Windows Terminal windows that are open.
pub fn terminal_windows() -> Vec<isize> {
    svc_windows::windows()
        .into_iter()
        .filter(|w| w.exe.to_lowercase().ends_with("windowsterminal.exe"))
        .map(|w| w.hwnd)
        .collect()
}
