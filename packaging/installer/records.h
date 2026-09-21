#ifndef PANPDF_RECORDS_H
#define PANPDF_RECORDS_H

#include <windows.h>
#include <wchar.h>

/* `installed-files.txt`, the only record of what an installation put on the
 * machine. One line per thing, in the order it was created:
 *
 *     F<path relative to the install folder>
 *     L<absolute path of a shortcut>
 *
 * written as UTF-8 by the installer and read back by both the installer -- when
 * it is replacing an older version -- and the uninstaller. Neither ever deletes
 * anything that is not named in it, which is why a file the person put in the
 * install folder themselves survives both.
 *
 * `records_read` returns a newly allocated wide string, or NULL when there is
 * no record; free it with `records_free`. `records_delete` walks it, calling
 * `saying` before each one if it is not NULL. */

typedef void (*records_saying)(void *context, const wchar_t *what, int done, int total);

wchar_t *records_read(const wchar_t *folder);
void records_free(wchar_t *lines);
int records_count(const wchar_t *lines);
int records_delete(const wchar_t *folder, wchar_t *lines, records_saying saying, void *context);

/* Remove every folder under `root` that is empty now the files have gone,
 * deepest first. `RemoveDirectoryW` refuses a folder that still holds
 * anything, which is exactly the behaviour wanted. */
void records_prune(const wchar_t *root);

#endif
