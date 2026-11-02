// What to tell the user when a page tool fails. Errors of opening and saving keep their own words
// (lib/errors.ts); these are the ones that only the page tools give.

import { describeError, type AppErrorLike } from "./errors";

export interface PageErrorText {
  title: string;
  detail: string;
}

/** Words for an error of a page tool. `name` is the document, as a person knows it. */
export function describePageError(error: AppErrorLike, name: string): PageErrorText {
  switch (error.code) {
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
