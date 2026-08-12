"""Create the deterministic demo knowledge system in an initialized installation."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from sqlmodel import Session, select  # noqa: E402

from app.db.database import engine, init_db  # noqa: E402
from app.db.models import User  # noqa: E402
from app.demo import seed  # noqa: E402


def main() -> None:
    init_db()
    with Session(engine) as session:
        owner = session.exec(select(User).where(User.is_admin == True)).first()  # noqa: E712
        if not owner:
            raise SystemExit("Initialize OntoPilot once so the admin account exists, then rerun this command")
        ks = seed(session, owner, force=True)
        print(f"Demo ready: {ks.name} (knowledge system {ks.id})")


if __name__ == "__main__":
    main()
