// Turns an error from the engine into words the user can act on.

export interface AppErrorLike {
  code: string;
  message: string;
}

export interface Described {
  title: string;
  detail: string;
  /** What the dialog should offer. */
  action: "none" | "password" | "retry";
}

/** The shape Tauri rejects with when a command fails. */
export function toAppError(error: unknown): AppErrorLike {
  if (typeof error === "object" && error !== null) {
    const e = error as { code?: unknown; message?: unknown };
    if (typeof e.code === "string") {
      return { code: e.code, message: typeof e.message === "string" ? e.message : "" };
    }
    if (error instanceof Error) return { code: "internal", message: error.message };
  }
  return { code: "internal", message: typeof error === "string" ? error : "Unknown error" };
}

/** Errors that mean the engine process was lost and the open document with it. */
export function isEngineLost(error: AppErrorLike): boolean {
  return error.code === "engine_stopped" || error.code === "unknown_document" || error.code === "save_took_too_long";
}

export function describeError(error: AppErrorLike, fileName?: string): Described {
  const file = fileName ? `"${fileName}"` : "This file";
  switch (error.code) {
    case "password_required":
      return {
        title: "This PDF is password protected",
        detail: `${file} needs a password before it can be opened.`,
        action: "password",
      };
    case "password_incorrect":
      return {
        title: "That password is not correct",
        detail: "Check the password and try again.",
        action: "password",
      };
    case "corrupt":
      return {
        title: "This file cannot be opened",
        detail: `${file} is damaged or is not a PDF file.`,
        action: "none",
      };
    case "io":
      return {
        title: "The file cannot be read",
        detail: `${file} could not be read. Check that it still exists and that you are allowed to open it.`,
        action: "none",
      };
    case "unsupported":
      return {
        title: "This PDF cannot be opened yet",
        detail: `${file} uses a kind of protection that Patient Partridge does not support.`,
        action: "none",
      };
    case "engine_stopped":
      return {
        title: "The PDF engine stopped",
        detail: `The part of Patient Partridge that reads PDF files stopped while working on ${fileName ? file : "this file"}. The file may be damaged. The app is still running, so you can try again or open another file.`,
        action: "retry",
      };
    case "save_took_too_long":
      return {
        title: "The save took too long",
        detail: `The PDF engine was still working on ${fileName ? file : "this file"} when the time allowed for a file of that size ran out, so it was stopped. The file on the disk was not changed, but the changes made since it was opened are lost. A very slow drive or network connection can cause this. Open the file again, and save a copy to a local disk if it happens again.`,
        action: "retry",
      };
    case "unknown_document":
      return {
        title: "The PDF engine was restarted",
        detail: "The document was lost when the PDF engine restarted. Open the file again to continue.",
        action: "retry",
      };
    case "engine_unavailable":
      return {
        title: "The PDF engine cannot start",
        detail: `The PDF library is missing or damaged. Reinstalling Patient Partridge should fix this.${error.message ? ` (${error.message})` : ""}`,
        action: "none",
      };
    case "sandbox_unavailable":
      return {
        title: "The file was not opened",
        detail: `Patient Partridge could not lock its PDF reader into the protective sandbox of this system, so it did not open ${fileName ? file : "this file"}. No part of the file was read.${error.message ? ` (${error.message})` : ""}`,
        action: "none",
      };
    case "save_read_only":
      return {
        title: "The file cannot be saved here",
        detail: `You may not change ${file}: it is read-only, or the rights on it do not let you write to it. Nothing was changed. Save a copy under another name instead.`,
        action: "none",
      };
    case "save_disk_full":
      return {
        title: "The disk is full",
        detail: `There is not enough room to save ${file}. The file on the disk was not changed. Free some space and save again.`,
        action: "none",
      };
    case "save_write_failed":
      return {
        title: "The file could not be saved",
        detail: `${file} was not changed.${error.message ? ` (${error.message})` : ""} Your changes are still in the open document.`,
        action: "none",
      };
    case "save_verify_failed":
      return {
        title: "The saved file did not pass the check",
        detail: `Patient Partridge wrote the new file, opened it again to check it, and found that it is not right, so it was not used. ${file} was not changed, and your changes are still in the open document. Try saving a copy under another name.`,
        action: "none",
      };
    case "save_protection_lost":
      return {
        title: "The password protection would be lost",
        detail: `The new file would not have been protected by the password like ${file} is, so it was not used. ${file} was not changed, and your changes are still in the open document.`,
        action: "none",
      };
    case "save_source_changed":
      return {
        title: "The file changed while it was open",
        detail: `Another program changed ${file} in place while Patient Partridge had it open. The app reads a PDF as it needs it, so what it has now would be a mix of the old and the new file, and nothing was saved. The file on the disk was not touched. Close the tab and open the file again to keep working. Changes made here since it was opened cannot be saved.`,
        action: "none",
      };
    case "save_source_touched":
      return {
        title: "The file was touched while it was open",
        detail: `${file} got a new time of change while Patient Partridge had it open. Nothing seen differs, but parts of it had not been read yet, so the app cannot be sure that what it has is the file, and it does not replace it. Nothing was saved, and the file on the disk was not touched. Save a copy: it holds what the app had read, and your changes are still in the open document.`,
        action: "none",
      };
    case "save_source_unreadable":
      return {
        title: "The file could not be read",
        detail: `A part of ${file} could not be read while saving, so the saved file would be missing parts. A drive or a network connection may have dropped, or the file may be damaged in a place that only shows when it is saved. Nothing was saved, and the file on the disk was not touched. Close the tab, check the drive, and open the file again.`,
        action: "none",
      };
    case "save_locked":
      return {
        title: "The file cannot be replaced here",
        detail: `${file} is open in a way that does not let it be replaced. This is usual on a USB stick or a network drive: the app keeps the file open while the document is open. Nothing was changed. Save a copy instead.`,
        action: "none",
      };
    case "save_hard_link":
      return {
        title: "The file has more than one name",
        detail: `${file} is a hard link: the same file under several names. Saving would give this name a new file and leave the other names with the old one, so nothing was changed. Save a copy instead.`,
        action: "none",
      };
    case "save_owner_changes":
      return {
        title: "The file belongs to someone else",
        detail: `${file} belongs to another user or group, and the new file would not. Saving would take it from them, so nothing was changed. Save a copy instead.`,
        action: "none",
      };
    case "save_access_kept":
      return {
        title: "The access rights could not be kept",
        detail: `The access rights of ${file} could not be given to the new file, so nothing was changed. Saving would have changed who may open the file. Save a copy instead.`,
        action: "none",
      };
    case "save_not_a_file":
      return {
        title: "That is not a file",
        detail: "The place you chose is a folder or something else that cannot hold a PDF. Nothing was saved.",
        action: "none",
      };
    case "save_busy":
      return {
        title: "Already saving",
        detail: `${file} is being saved. Wait for that to finish.`,
        action: "none",
      };
    case "save_failed":
      return {
        title: "The document could not be written",
        detail: `The PDF engine could not write ${file} out. Nothing was saved, and the file on the disk was not changed. Your changes are still in the open document.`,
        action: "none",
      };
    case "save_too_large":
      return {
        title: "The document is too big to save",
        detail: `${file} would be larger than 512 MiB, which is more than Patient Partridge saves. Nothing was saved, and the file on the disk was not changed.`,
        action: "none",
      };
    case "updating":
      return {
        title: "An update is being installed",
        detail: "Patient Partridge cannot open files until it has restarted. Restart the app, then open the file again.",
        action: "none",
      };
    default:
      return {
        title: "Something went wrong",
        detail: error.message || "An unexpected error happened.",
        action: "none",
      };
  }
}

/** Last part of a path, for window titles and messages. Handles both kinds of separator. */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
