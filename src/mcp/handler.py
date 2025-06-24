"""Enhanced MCP handler with fantasy football context."""

from typing import Any, Dict, List, Optional, Union

from fastapi import HTTPException
from structlog import get_logger

from .context import FantasyFootballContext
from .models import (
    MCPFunction,
    MCPFunctionParameter,
    MCPInvocation,
    MCPResponse,
    MCPResponseStatus,
)
from ..services import SleeperAPIClient

logger = get_logger(__name__)


class EnhancedMCPHandler:
    """Enhanced handler for MCP protocol operations with fantasy context."""

    def __init__(self, client: SleeperAPIClient):
        """Initialize the MCP handler.
        
        Args:
            client: Sleeper API client instance
        """
        self.client = client
        self.context = FantasyFootballContext()

    async def execute_function(self, invocation: MCPInvocation) -> MCPResponse:
        """Execute an MCP function invocation with enhanced context.
        
        Args:
            invocation: Function invocation request
            
        Returns:
            MCPResponse: Result of the function execution with additional context
            
        Raises:
            HTTPException: If the function execution fails
        """
        try:
            # Get the corresponding method from the client
            method = getattr(self.client, invocation.function_name, None)
            if not method:
                raise ValueError(f"Unknown function: {invocation.function_name}")

            # Execute the function
            logger.info(
                "executing_function",
                function=invocation.function_name,
                parameters=invocation.parameters,
            )
            result = await method(**invocation.parameters)

            # Enhance the result with additional context
            enhanced_result = self._enhance_result(
                invocation.function_name,
                result,
                invocation.parameters
            )

            # Convert the result to a serializable format if needed
            if hasattr(enhanced_result, "model_dump"):
                enhanced_result = enhanced_result.model_dump()
            elif isinstance(enhanced_result, list) and enhanced_result and hasattr(enhanced_result[0], "model_dump"):
                enhanced_result = [item.model_dump() for item in enhanced_result]

            return MCPResponse(
                status=MCPResponseStatus.SUCCESS,
                result=enhanced_result,
            )

        except HTTPException as e:
            logger.error(
                "function_execution_failed",
                function=invocation.function_name,
                status_code=e.status_code,
                detail=e.detail,
            )
            return MCPResponse(
                status=MCPResponseStatus.ERROR,
                error=str(e.detail),
            )
        except Exception as e:
            logger.exception(
                "unexpected_error",
                function=invocation.function_name,
                error=str(e),
            )
            return MCPResponse(
                status=MCPResponseStatus.ERROR,
                error=f"Internal error: {str(e)}",
            )

    def _enhance_result(
        self,
        function_name: str,
        result: Any,
        parameters: Dict[str, Any]
    ) -> Any:
        """Enhance API results with additional context.
        
        Args:
            function_name: Name of the executed function
            result: Original API result
            parameters: Function parameters
            
        Returns:
            Any: Enhanced result with additional context
        """
        # Convert result to dict if it's a model
        result_data = result.model_dump() if hasattr(result, "model_dump") else result

        if isinstance(result_data, dict):
            context_data = {}

            if function_name == "get_league":
                # Add league type information and strategies
                league_type = self.context.get_league_type(
                    "ppr" if result_data.get("scoring_settings", {}).get("reception", 0) > 0
                    else "standard"
                )
                if league_type:
                    context_data["league_type_info"] = league_type.model_dump()
                
                # Add strategy suggestions
                context_data["suggested_strategies"] = self.context.suggest_strategies(result_data)

            elif function_name == "get_league_rosters":
                # Add position information for roster slots
                if "roster_positions" in result_data:
                    position_info = {}
                    for pos in result_data["roster_positions"]:
                        pos_data = self.context.get_position_info(pos)
                        if pos_data:
                            position_info[pos] = pos_data.model_dump()
                    context_data["position_info"] = position_info

            # Merge context data with original result
            if context_data:
                if isinstance(result_data, dict):
                    result_data["context"] = context_data
                else:
                    result_data = {"data": result_data, "context": context_data}

        elif isinstance(result_data, list):
            # Handle list results (e.g., multiple players or rosters)
            enhanced_list = []
            for item in result_data:
                if isinstance(item, dict):
                    enhanced_item = item.copy()
                    # Add relevant context based on item type
                    if "position" in item:
                        pos_info = self.context.get_position_info(item["position"])
                        if pos_info:
                            enhanced_item["position_info"] = pos_info.model_dump()
                    enhanced_list.append(enhanced_item)
                else:
                    enhanced_list.append(item)
            result_data = enhanced_list

        return result_data


def get_enhanced_functions() -> List[MCPFunction]:
    """Get the enhanced list of available MCP functions.
    
    Returns:
        List[MCPFunction]: List of available functions with enhanced descriptions
    """
    return [
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
            description="Get all leagues for a specific user with league type information and strategy suggestions",
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
            description=(
                "Get detailed information about a specific league, including league type, "
                "scoring explanations, and strategy suggestions"
            ),
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
            description=(
                "Get all rosters in a league with position information and "
                "scoring potential for each roster slot"
            ),
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
