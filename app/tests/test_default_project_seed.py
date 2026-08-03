"""A fresh install must come with a project already in place.

Processes can run unassigned, but project-scoped features (assistant profiles, the
"Continue planning" pointer) no-op without a project row, so a new install would need the
user to create one by hand before those work.
"""

from sqlmodel import Session, select

from database import create_db_and_tables
from models import Project, Workspace


def _projects(engine) -> list[Project]:
    with Session(engine) as session:
        return list(session.exec(select(Project)).all())


def test_fresh_database_gets_one_starter_project(test_engine):
    projects = _projects(test_engine)
    assert len(projects) == 1
    assert projects[0].name == "My Project"


def test_seeding_again_does_not_add_a_second_project(test_engine):
    create_db_and_tables()
    assert len(_projects(test_engine)) == 1


def test_existing_project_is_left_alone(test_engine):
    with Session(test_engine) as session:
        session.exec(select(Project)).one().name = "Renamed"
        session.add(Project(name="Mine"))
        session.commit()

    create_db_and_tables()

    names = sorted(p.name for p in _projects(test_engine))
    assert names == ["Mine", "Renamed"]


def test_starter_project_joins_the_default_workspace(test_engine):
    """Stranded outside every workspace, the project is invisible to a workspace-scoped token."""
    with Session(test_engine) as session:
        workspace = session.exec(select(Workspace).where(Workspace.slug == "default")).one()
        assert session.exec(select(Project)).one().workspace_id == workspace.id
