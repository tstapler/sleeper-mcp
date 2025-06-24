"""Main FastAPI application for the Sleeper MCP server."""

from contextlib import asynccontextmanager
from typing import AsyncGenerator

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from structlog import get_logger

from src.config import get_config
from src.mcp import (
    MCPCapabilities,
    MCPHandler,
    MCPInvocation,
    MCPResponse,
    get_enhanced_functions,
)
from src.services import SleeperAPIClient

logger = get_logger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncGenerator[None, None]:
    """Manage application lifespan and dependencies.
    
    Args:
        app: FastAPI application instance
    """
    # Create API client and MCP handler
    config = get_config()
    app.state.sleeper_client = SleeperAPIClient(config)
    app.state.mcp_handler = MCPHandler(app.state.sleeper_client)
    logger.info("application_startup", config=config.model_dump())
    
    yield
    
    # Cleanup
    await app.state.sleeper_client.close()
    logger.info("application_shutdown")


app = FastAPI(
    title="Sleeper MCP Server",
    description=(
        "Machine Context Provider server for Sleeper fantasy sports data. "
        "Provides enhanced context and insights for fantasy football operations."
    ),
    version="0.1.0",
    lifespan=lifespan,
)

# Add CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/health")
async def health_check() -> dict:
    """Check application health.
    
    Returns:
        dict: Health check response with API connectivity status
    """
    try:
        # Test API connection
        await app.state.sleeper_client.get_nfl_state()
        return {
            "status": "healthy",
            "message": "Application is running and can connect to Sleeper API",
        }
    except Exception as e:
        logger.error("health_check_failed", error=str(e))
        return {
            "status": "unhealthy",
            "message": f"Health check failed: {str(e)}",
        }


@app.get("/capabilities", response_model=MCPCapabilities)
async def get_capabilities() -> dict:
    """Get the MCP server capabilities.
    
    Returns:
        MCPCapabilities: Server capabilities including available functions with
        enhanced descriptions and context information
    """
    return {
        "functions": get_enhanced_functions(),
        "version": "1.0",
        "provider": "sleeper",
    }


@app.post("/invoke", response_model=MCPResponse)
async def invoke_function(invocation: MCPInvocation) -> MCPResponse:
    """Invoke an MCP function with enhanced context.
    
    Args:
        invocation: Function invocation request
        
    Returns:
        MCPResponse: Result of the function execution with additional fantasy
        football context and insights
    """
    return await app.state.mcp_handler.execute_function(invocation)
