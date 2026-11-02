// What to tell the user when a page tool fails. Errors of opening and saving keep their own words
// (lib/errors.ts); these are the ones that only the page tools give.

import { describeError, type AppErrorLike } from "./errors";

function capital(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export interface PageErrorText {
  title: string;
  detail: string;
}

/**
 * Words for an error of a page tool. `name` is the document, as a person knows it. `source` is the
 * other file, when pages were being added from one: what went wrong with that file is told about
 * that file, and the document is said to be as it was.
 */
export function describePageError(error: AppErrorLike, name: string, source?: string): PageErrorText {
  if (source !== undefined) {
    switch (error.code) {
      case "insert_source_failed":
      case "import_failed":
      case "corrupt":
      case "io":
      case "unsupported":
      case "too_many_pages":
        return {
          title: "The pages could not be added",
          detail: `"${source}" could not be used${error.message ? ` (${error.message})` : ""}. "${name}" was not changed.`,
        };
      default:
        break;
    }
  }
  switch (error.code) {
    case "insert_source_failed":
      return {
        title: "The pages could not be added",
        detail: `The other file could not be used${error.message ? ` (${error.message})` : ""}. "${name}" was not changed.`,
      };
    case "pages_not_allowed":
      return {
        title: "This document does not allow that",
        detail: `${error.message ? `${capital(error.message)}.` : "The permissions of the document do not allow this change."} "${name}" was not changed.`,
      };
    case "source_changed":
    case "source_unreadable":
      return {
        title: "The file changed while it was open",
        detail: `"${name}" was changed by something else after it was opened, or a part of it could not be read, so the page tools cannot be used on it safely. Nothing was changed. Close it and open it again.`,
      };
    case "too_many_pages":
      return {
        title: "Too many pages for this tool",
        detail: `The page tools work on documents of up to 10,000 pages, and on up to 10,000 pages at a time. "${name}" can still be read, searched and saved.`,
      };
    case "snapshot_gone":
      return {
        title: "That change cannot be taken back any more",
        detail:
          "The app keeps a few copies of the document to take page changes back, and this one was let go of to make room. The document is as it is now.",
      };
    case "import_failed":
      return {
        title: "The pages could not be added",
        detail: `The other file could not be used${error.message ? ` (${error.message})` : ""}. "${name}" was not changed.`,
      };
    case "page_change_failed":
      return {
        title: "The pages could not be changed",
        detail: `The PDF library refused the change${error.message ? ` (${error.message})` : ""}. "${name}" was not changed.`,
      };
    case "busy":
    case "save_busy":
      return {
        title: "Wait a moment",
        detail: `"${name}" is being saved, or another page tool is still working on it. Try again when it is done.`,
      };
    case "extract_over_source":
      return {
        title: "Choose another name",
        detail: `The new file would replace "${name}" itself. Pick a different name or folder; the document is not changed.`,
      };
    case "invalid_request":
    case "page_out_of_range":
      return {
        title: "That change is not possible",
        detail: `${error.message || "The pages that were named cannot be changed that way."} "${name}" was not changed.`,
      };
    case "undo_unavailable":
      return {
        title: "That change cannot be taken back",
        detail:
          "The document was too big for the app to keep a copy of it, so this change cannot be undone. Close the document without saving to go back to the file as it is on the disk.",
      };
    default: {
      const text = describeError(error, name);
      return { title: text.title, detail: text.detail };
    }
  }
}
