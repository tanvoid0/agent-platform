import logging
import os
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, HTMLResponse, RedirectResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates

from api_auth import verify_agent_platform_api_key
from chat_routes import router as chat_router
from database import create_db_and_tables
from orchestrator_env import orchestrator_master_key
from process_routes import router as process_router
from projects_routes import router as projects_router
from teams_routes import router as teams_router
from workspace_routes import router as workspace_router

logger = logging.getLogger(__name__)

app = FastAPI(title="Agent Platform", version="0.1.0")

_api_deps = [Depends(verify_agent_platform_api_key)]
app.include_router(process_router, dependencies=_api_deps)
app.include_router(teams_router, dependencies=_api_deps)
app.include_router(projects_router, dependencies=_api_deps)
app.include_router(workspace_router, dependencies=_api_deps)
app.include_router(process_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(teams_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(projects_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(workspace_router, prefix="/api/v1", dependencies=_api_deps)
app.include_router(chat_router, prefix="/api/v1", dependencies=_api_deps)

app.add_middleware(
    CORSMiddleware,
    allow_origins=os.getenv("CORS_ALLOW_ORIGINS", "*").split(","),
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

BASE_DIR = Path(__file__).resolve().parent
templates = Jinja2Templates(directory=str(BASE_DIR / "templates"))
_static_dir = BASE_DIR / "static" / "dist"

_FLOW_STATIC_FILE_EXT = frozenset(
    {
        "css",
        "eot",
        "html",
        "ico",
        "jpeg",
        "jpg",
        "js",
        "json",
        "map",
        "png",
        "svg",
        "ttf",
        "txt",
        "webp",
        "woff",
        "woff2",
    }
)


@app.middleware("http")
async def flow_spa_fallback(request: Request, call_next):
    """Serve `index.html` for client-side routes under `/flow` when the Vite build is present."""
    response = await call_next(request)
    if response.status_code != 404:
        return response
    path = request.url.path
    if not path.startswith("/flow/"):
        return response
    if path.startswith("/flow/assets/"):
        return response
    tail = path.rsplit("/", 1)[-1]
    if "." in tail:
        ext = tail.rsplit(".", 1)[-1].lower()
        if ext in _FLOW_STATIC_FILE_EXT:
            return response
    index_file = _static_dir / "index.html"
    if not index_file.is_file():
        return response
    return FileResponse(index_file)


@app.get("/flow", include_in_schema=False)
def redirect_flow_slash():
    # StaticFiles + Vite `base: "/flow/"` expect `/flow/`; bare `/flow` is often a 404.
    return RedirectResponse(url="/flow/", status_code=307)


if _static_dir.is_dir():
    app.mount(
        "/flow",
        StaticFiles(directory=str(_static_dir), html=True),
        name="flow",
    )
else:

    @app.get("/flow/", include_in_schema=False)
    def flow_not_built():
        return HTMLResponse(
            "<p>Build the web UI: <code>cd web && npm install && npm run build</code></p>",
            status_code=503,
        )


@app.get("/", include_in_schema=False)
def root():
    return RedirectResponse(url="/ui")


@app.get("/health")
def health():
    return {"status": "ok", "service": "agent-platform"}


@app.on_event("startup")
def on_startup():
    create_db_and_tables()
    if not orchestrator_master_key():
        logger.warning(
            "ORCHESTRATOR_MASTER_KEY is not set; LLM calls will fail until it matches llm-orchestrator."
        )


@app.get("/ui", response_class=HTMLResponse, include_in_schema=False)
def ui_page(request: Request):
    return templates.TemplateResponse(
        "index.html",
        {
            "request": request,
            "api_base": os.getenv("AGENT_PLATFORM_PUBLIC_API", "").strip() or "",
        },
    )


@app.get("/api-guide", response_class=HTMLResponse, include_in_schema=False)
def api_guide_page(request: Request):
    return templates.TemplateResponse("api_guide.html", {"request": request})
