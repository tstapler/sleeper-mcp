"""Function specifications for the Sleeper MCP.

This module defines the available functions that can be called through the MCP protocol.
Each function is mapped to corresponding Sleeper API operations.
"""

from typing import List

from .models import MCPFunction, MCPFunctionParameter

SLEEPER_FUNCTIONS: List[MCPFunction] = [
    MCPFunction(
        name="get_user",
        description="Get information about a Sleeper user by username or user ID",
        parameters=[
            MCPFunctionParameter(
                name="identifier",
                type="string",
                description="Username or user_id of the user to look up",
                required=True,
            ),
        ],
    ),
    MCPFunction(
        name="get_user_leagues",
        description="Get all leagues for a specific user in a given season",
        parameters=[
            MCPFunctionParameter(
                name="user_id",
                type="string",
                description="User ID to look up leagues for",
                required=True,
            ),
            MCPFunctionParameter(
                name="season",
                type="string",
                description="Season year (e.g., '2023')",
                required=True,
            ),
            MCPFunctionParameter(
                name="sport",
                type="string",
                description="Sport type",
                required=False,
                default="nfl",
            ),
        ],
    ),
    MCPFunction(
        name="get_league",
        description="Get detailed information about a specific league",
        parameters=[
            MCPFunctionParameter(
                name="league_id",
                type="string",
                description="League ID to look up",
                required=True,
            ),
        ],
    ),
    MCPFunction(
        name="get_league_rosters",
        description="Get all rosters in a specific league",
        parameters=[
            MCPFunctionParameter(
                name="league_id",
                type="string",
                description="League ID to look up rosters for",
                required=True,
            ),
        ],
    ),
    MCPFunction(
        name="get_league_users",
        description="Get all users in a specific league",
        parameters=[
            MCPFunctionParameter(
                name="league_id",
                type="string",
                description="League ID to look up users for",
                required=True,
            ),
        ],
    ),
    MCPFunction(
        name="get_nfl_state",
        description="Get current NFL season state information",
        parameters=[],
    ),
]


def get_mcp_capabilities() -> dict:
    """Get the MCP server capabilities.
    
    Returns:
        dict: Server capabilities including available functions
    """
    return {
        "functions": SLEEPER_FUNCTIONS,
        "version": "1.0",
        "provider": "sleeper",
    }
