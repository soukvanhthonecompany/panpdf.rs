/* The uninstaller that ships inside the installer's payload.
 *
 * It removes exactly what `installed-files.txt` beside it says was installed --
 * the files, then the shortcuts, then the one registry key -- and nothing else.
 * A file the person put in the folder afterwards is left where it is, because
 * only the recorded names are deleted and only empty folders are removed.
 *
 * Reading and undoing that record is `records.c`, which the installer uses too
 * when it is replacing an older version: the two must agree to the file about
 * what the old version put there.
 *
 * `uninstall.exe /quiet` skips the question and the closing word, which is what
 * the QuietUninstallString in the registry runs.
 *
 * Every window it puts up is topmost and asks for the foreground. Settings
 * starts the uninstaller and then keeps the foreground itself, so a question
 * that does not insist opens *behind* Settings and the person sees nothing
 * happen at all -- which is what this did until it was watched doing it. */

#include <windows.h>
#include <commctrl.h>
#include <shlwapi.h>
#include <wchar.h>
#include "records.h"

#define PRODUCT      L"PanPDF"
#define REGISTRY_KEY L"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PanPDF"

static HWND window, progress, status;
static wchar_t here[MAX_PATH];

static void tell(const wchar_t *line, int done, int total)
{
    if (!window) return;
    SetWindowTextW(status, line);
    SendMessageW(progress, PBM_SETRANGE32, 0, (LPARAM)(total > 0 ? total : 1));
    SendMessageW(progress, PBM_SETPOS, (WPARAM)done, 0);
    MSG message;
    while (PeekMessageW(&message, NULL, 0, 0, PM_REMOVE)) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

static void say_one(void *context, const wchar_t *what, int done, int total)
{
    (void)context;
    tell(what, done, total);
}

static LRESULT CALLBACK proc(HWND hwnd, UINT message, WPARAM wp, LPARAM lp)
{
    if (message == WM_CTLCOLORSTATIC) {
        SetBkMode((HDC)wp, TRANSPARENT);
        return (LRESULT)GetSysColorBrush(COLOR_WINDOW);
    }
    if (message == WM_ERASEBKGND) {
        RECT area;
        GetClientRect(hwnd, &area);
        FillRect((HDC)wp, &area, GetSysColorBrush(COLOR_WINDOW));
        return 1;
    }
    if (message == WM_CLOSE) return 0; /* removal is quick; do not leave it half done */
    return DefWindowProcW(hwnd, message, wp, lp);
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR command, int show)
{
    (void)previous; (void)show;
    int quiet = command && wcsstr(command, L"/quiet") != NULL;

    GetModuleFileNameW(NULL, here, MAX_PATH);
    wchar_t *slash = wcsrchr(here, L'\\');
    if (slash) *slash = 0;

    if (!quiet) {
        wchar_t question[MAX_PATH + 128];
        swprintf(question, MAX_PATH + 128,
                 L"Remove PanPDF from\n%ls ?\n\nYour PDF files are not touched.", here);
        if (MessageBoxW(NULL, question, PRODUCT,
                        MB_OKCANCEL | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST) != IDOK)
            return 0;
    }

    wchar_t *lines = records_read(here);
    if (!lines) {
        if (!quiet)
            MessageBoxW(NULL, L"installed-files.txt is missing, so there is no record of what "
                              L"was installed. Nothing has been deleted.", PRODUCT,
                        MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST);
        return 1;
    }
    int total = records_count(lines);

    if (!quiet) {
        INITCOMMONCONTROLSEX controls = { sizeof controls, ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES };
        InitCommonControlsEx(&controls);
        WNDCLASSEXW info;
        memset(&info, 0, sizeof info);
        info.cbSize = sizeof info;
        info.lpfnWndProc = proc;
        info.hInstance = instance;
        info.hCursor = LoadCursorW(NULL, IDC_ARROW);
        info.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
        info.lpszClassName = L"PanPDFUninstaller";
        RegisterClassExW(&info);
        window = CreateWindowExW(WS_EX_TOPMOST, L"PanPDFUninstaller", L"Removing " PRODUCT,
                                 WS_OVERLAPPED | WS_CAPTION, CW_USEDEFAULT, CW_USEDEFAULT,
                                 460, 150, NULL, NULL, instance, NULL);
        status = CreateWindowExW(0, L"STATIC", L"", WS_CHILD | WS_VISIBLE | SS_PATHELLIPSIS,
                                 20, 18, 410, 20, window, NULL, instance, NULL);
        progress = CreateWindowExW(0, PROGRESS_CLASSW, L"", WS_CHILD | WS_VISIBLE | PBS_SMOOTH,
                                   20, 48, 410, 20, window, NULL, instance, NULL);
        HFONT font = CreateFontW(-13, 0, 0, 0, FW_NORMAL, 0, 0, 0, DEFAULT_CHARSET,
                                 OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                                 DEFAULT_PITCH | FF_DONTCARE, L"Segoe UI");
        SendMessageW(status, WM_SETFONT, (WPARAM)font, TRUE);
        ShowWindow(window, SW_SHOWNORMAL);
        SetForegroundWindow(window);
        UpdateWindow(window);
    }

    records_delete(here, lines, say_one, NULL);
    records_free(lines);

    tell(L"Removing the entry in Add or remove programs", total, total);
    RegDeleteKeyW(HKEY_CURRENT_USER, REGISTRY_KEY);

    records_prune(here);

    if (!quiet) {
        DestroyWindow(window);
        window = NULL;
        MessageBoxW(NULL, L"PanPDF has been removed.", PRODUCT,
                    MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST);
    }

    /* Last of all, this file and the folder it sits in. A running program
     * cannot delete itself, so a detached `cmd` waits a moment and does it.
     * `rd` without `/s` refuses a folder that still holds something, so a file
     * the person put there themselves keeps the folder and keeps itself. */
    wchar_t order[MAX_PATH * 2 + 128];
    swprintf(order, MAX_PATH * 2 + 128,
             L"/c ping -n 3 127.0.0.1 >nul & del /f /q \"%ls\\uninstall.exe\" & rd \"%ls\"",
             here, here);
    ShellExecuteW(NULL, L"open", L"cmd.exe", order, NULL, SW_HIDE);
    return 0;
}
