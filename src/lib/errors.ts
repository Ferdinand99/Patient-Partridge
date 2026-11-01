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
  return error.code === "engine_stopped" || error.code === "unknown_document";
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
