"""Checks the one signature of a PDF with pyHanko, an independent reader of signed PDFs.

usage: python -I pyhanko_check.py <signed.pdf> <root.pem>

Prints one line of JSON: whether the signature is intact, valid and trusted (the root is given as the
only trust anchor), what part of the file it covers, and what kind of change followed it.
"""

import json
import sys

from pyhanko.keys import load_cert_from_pemder
from pyhanko.pdf_utils.reader import PdfFileReader
from pyhanko.sign.validation import validate_pdf_signature
from pyhanko_certvalidator import ValidationContext


def main(path, root):
    context = ValidationContext(trust_roots=[load_cert_from_pemder(root)], allow_fetching=False)
    with open(path, "rb") as f:
        reader = PdfFileReader(f)
        signatures = reader.embedded_signatures
        if len(signatures) != 1:
            print(json.dumps({"error": f"{len(signatures)} signatures"}))
            return 1
        embedded = signatures[0]
        status = validate_pdf_signature(embedded, context)
        modification = status.modification_level
        print(
            json.dumps(
                {
                    "intact": bool(status.intact),
                    "valid": bool(status.valid),
                    "trusted": bool(status.trusted),
                    "coverage": status.coverage.name,
                    "modification_level": modification.name if modification else None,
                    "sub_filter": str(embedded.sig_object.get("/SubFilter")),
                }
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
