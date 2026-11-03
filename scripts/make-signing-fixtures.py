#!/usr/bin/env python3
"""Makes the test certificate files in crates/signer/tests/fixtures.

Every key and certificate here is made for the tests and is not a secret: the password of every
file is the word `test` (or empty where the name says so). The certificate authority that issued
them is made again on every run, and its key is thrown away. Nothing here is a real identity.

Run it in a place that has Python with the `cryptography` package and the `openssl` program:

    python3 scripts/make-signing-fixtures.py crates/signer/tests/fixtures

The files that come out are committed, so the tests need neither Python nor OpenSSL. They have to
be made again only when a test needs a kind of file that is not there.
"""

import datetime
import subprocess
import sys
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519, rsa
from cryptography.hazmat.primitives.serialization import pkcs12
from cryptography.x509.oid import NameOID

PASSWORD = b"test"
UTC = datetime.timezone.utc


def when(year, month=1, day=1):
    return datetime.datetime(year, month, day, tzinfo=UTC)


def name(common, org="Partridge Test Certificates"):
    return x509.Name(
        [
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, org),
            x509.NameAttribute(NameOID.COMMON_NAME, common),
        ]
    )


def key_usage(**flags):
    base = dict(
        digital_signature=False,
        content_commitment=False,
        key_encipherment=False,
        data_encipherment=False,
        key_agreement=False,
        key_cert_sign=False,
        crl_sign=False,
        encipher_only=False,
        decipher_only=False,
    )
    base.update(flags)
    return x509.KeyUsage(**base)


def certificate(subject, subject_key, issuer_name, issuer_key, serial, not_before, not_after,
                ca=False, usage="sign", path_length=None):
    builder = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(issuer_name)
        .public_key(subject_key.public_key())
        .serial_number(serial)
        .not_valid_before(not_before)
        .not_valid_after(not_after)
        .add_extension(x509.SubjectKeyIdentifier.from_public_key(subject_key.public_key()), False)
    )
    if ca:
        builder = builder.add_extension(x509.BasicConstraints(ca=True, path_length=path_length), True)
        builder = builder.add_extension(key_usage(key_cert_sign=True, crl_sign=True), True)
    else:
        builder = builder.add_extension(x509.BasicConstraints(ca=False, path_length=None), True)
        if usage == "sign":
            builder = builder.add_extension(key_usage(digital_signature=True, content_commitment=True), True)
        elif usage == "commitment":
            builder = builder.add_extension(key_usage(content_commitment=True), True)
        elif usage == "encrypt":
            builder = builder.add_extension(key_usage(key_encipherment=True), True)
        # usage == "none": no key usage extension at all
    algorithm = None if isinstance(issuer_key, ed25519.Ed25519PrivateKey) else hashes.SHA256()
    return builder.sign(issuer_key, algorithm)


def write(out, file, key, cert, chain, encryption=None, password=PASSWORD):
    encryption = encryption or serialization.BestAvailableEncryption(password)
    data = pkcs12.serialize_key_and_certificates(b"test signer", key, cert, chain, encryption)
    (out / file).write_bytes(data)
    print(f"{file}: {len(data)} bytes")


def legacy():
    # The older form that Windows and macOS still export: triple DES and a SHA-1 MAC.
    return (
        serialization.PrivateFormat.PKCS12.encryption_builder()
        .kdf_rounds(2048)
        .key_cert_algorithm(pkcs12.PBES.PBESv1SHA1And3KeyTripleDESCBC)
        .hmac_hash(hashes.SHA1())
        .build(PASSWORD)
    )


def main(out):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    long_ago, far_off = when(2020), when(2120)

    root_key = rsa.generate_private_key(65537, 3072)
    root_name = name("Partridge Test Root")
    root = certificate(root_name, root_key, root_name, root_key, 1, long_ago, far_off, ca=True)
    mid_key = rsa.generate_private_key(65537, 3072)
    mid_name = name("Partridge Test Intermediate")
    mid = certificate(mid_name, mid_key, root_name, root_key, 2, long_ago, far_off, ca=True, path_length=0)
    (out / "test-root.pem").write_bytes(root.public_bytes(serialization.Encoding.PEM))
    (out / "test-intermediate.pem").write_bytes(mid.public_bytes(serialization.Encoding.PEM))

    serial = [100]

    def leaf(common, key, issuer="root", not_before=long_ago, not_after=far_off, usage="sign"):
        serial[0] += 1
        if issuer == "root":
            return certificate(name(common), key, root_name, root_key, serial[0], not_before, not_after, usage=usage)
        return certificate(name(common), key, mid_name, mid_key, serial[0], not_before, not_after, usage=usage)

    # Each key kind, with a chain of two (the root) or three (an intermediate too).
    k = rsa.generate_private_key(65537, 2048)
    c = leaf("Test Signer RSA 2048", k, issuer="mid")
    write(out, "rsa2048.p12", k, c, [mid, root])
    write(out, "rsa2048-legacy.p12", k, c, [mid, root], encryption=legacy())

    k = rsa.generate_private_key(65537, 3072)
    write(out, "rsa3072.p12", k, leaf("Test Signer RSA 3072", k), [root])
    k = rsa.generate_private_key(65537, 4096)
    write(out, "rsa4096.p12", k, leaf("Test Signer RSA 4096", k), [root])
    k = ec.generate_private_key(ec.SECP256R1())
    write(out, "p256.p12", k, leaf("Test Signer P-256", k), [root])
    c256 = leaf("Test Signer P-256 legacy", k)
    write(out, "p256-legacy.p12", k, c256, [root], encryption=legacy())
    k = ec.generate_private_key(ec.SECP384R1())
    write(out, "p384.p12", k, leaf("Test Signer P-384", k), [root])

    # Kinds of key that are refused.
    k = ed25519.Ed25519PrivateKey.generate()
    write(out, "ed25519.p12", k, leaf("Test Signer Ed25519", k), [root])
    k = ec.generate_private_key(ec.SECP521R1())
    write(out, "p521.p12", k, leaf("Test Signer P-521", k), [root])
    k = rsa.generate_private_key(65537, 1024)
    write(out, "rsa1024.p12", k, leaf("Test Signer RSA 1024", k), [root])

    # Certificates that are not what they should be (warned about, not refused).
    k = rsa.generate_private_key(65537, 2048)
    write(out, "rsa2048-expired.p12", k,
          leaf("Test Signer Expired", k, not_before=when(2015), not_after=when(2016)), [root])
    write(out, "rsa2048-not-yet.p12", k,
          leaf("Test Signer Not Yet", k, not_before=when(2090), not_after=when(2095)), [root])
    write(out, "rsa2048-encrypt-only.p12", k, leaf("Test Signer Encrypt Only", k, usage="encrypt"), [root])
    write(out, "rsa2048-commitment.p12", k, leaf("Test Signer Commitment Only", k, usage="commitment"), [root])
    write(out, "rsa2048-no-usage.p12", k, leaf("Test Signer No Usage Extension", k, usage="none"), [root])
    # A certificate that its holder made for themselves: no chain beyond itself.
    own_name = name("Test Signer Self Made")
    own = certificate(own_name, k, own_name, k, 7, long_ago, far_off)
    write(out, "rsa2048-self-signed.p12", k, own, [])
    # Names with letters the page font may not have.
    write(out, "rsa2048-nordic.p12", k, leaf("Åse Ødegård Test", k), [root])
    write(out, "rsa2048-cjk.p12", k, leaf("测试 Signer", k), [root])
    # No chain given at all: the file holds the holder's certificate only.
    write(out, "rsa2048-leaf-only.p12", k, leaf("Test Signer Leaf Only", k), [])

    # A file with certificates and no key (a trust store).
    data = pkcs12.serialize_key_and_certificates(b"roots", None, root, [mid],
                                                 serialization.BestAvailableEncryption(PASSWORD))
    (out / "no-key.p12").write_bytes(data)
    print(f"no-key.p12: {len(data)} bytes")

    # An empty password, made by the openssl program the way other tools make it.
    work = out / "work"
    work.mkdir(exist_ok=True)
    (work / "k.pem").write_bytes(k.private_bytes(serialization.Encoding.PEM,
                                                 serialization.PrivateFormat.PKCS8,
                                                 serialization.NoEncryption()))
    (work / "c.pem").write_bytes(leaf("Test Signer Empty Password", k).public_bytes(serialization.Encoding.PEM))
    subprocess.run(["openssl", "pkcs12", "-export", "-inkey", str(work / "k.pem"), "-in", str(work / "c.pem"),
                    "-certfile", str(out / "test-root.pem"), "-passout", "pass:",
                    "-out", str(out / "rsa2048-empty-password.p12")], check=True)
    for file in work.iterdir():
        file.unlink()
    work.rmdir()

def extra(out):
    """Files added later for tests of what is refused. They have a certificate authority of their
    own, so making them does not touch the files above: run this with `--extra` on its own."""
    out = Path(out)
    long_ago, far_off = when(2020), when(2120)
    # A chain of fourteen certificates: the holder's and thirteen authorities above it.
    authorities = []
    previous_key, previous_name = None, None
    for level in range(13):
        key = rsa.generate_private_key(65537, 2048)
        subject = name(f"Partridge Test Deep Authority {level}")
        if previous_key is None:
            cert = certificate(subject, key, subject, key, 1000 + level, long_ago, far_off, ca=True)
        else:
            cert = certificate(subject, key, previous_name, previous_key, 1000 + level, long_ago, far_off, ca=True)
        authorities.append((key, subject, cert))
        previous_key, previous_name = key, subject
    holder = rsa.generate_private_key(65537, 2048)
    holder_cert = certificate(name("Test Signer Deep Chain"), holder, previous_name, previous_key, 2000,
                              long_ago, far_off)
    chain = [cert for (_, _, cert) in reversed(authorities)]
    write(out, "rsa2048-chain14.p12", holder, holder_cert, chain)

    # The holder with the eleven nearest authorities is a chain of twelve, the most that is carried.
    write(out, "rsa2048-chain12.p12", holder, holder_cert, chain[:11])
    # An RSA key with the public exponent 3, which other programs do not accept.
    root_key = rsa.generate_private_key(65537, 2048)
    root_name = name("Partridge Test Small Exponent Root")
    root = certificate(root_name, root_key, root_name, root_key, 3000, long_ago, far_off, ca=True)
    key3 = rsa.generate_private_key(3, 2048)
    write(out, "rsa2048-exponent3.p12", key3,
          certificate(name("Test Signer Exponent 3"), key3, root_name, root_key, 3001, long_ago, far_off), [root])


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    folder = args[0] if args else "crates/signer/tests/fixtures"
    if "--extra" in sys.argv:
        extra(folder)
    else:
        main(folder)
