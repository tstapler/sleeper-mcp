"""Context providers for Sleeper fantasy football data.

This module provides additional context about fantasy football concepts and data
to help Goose better understand and interact with the domain.
"""

from typing import Any, Dict, List, Optional

from pydantic import BaseModel

class FantasyPosition(BaseModel):
    """Information about a fantasy football position."""
    
    code: str
    name: str
    description: str
    scoring_relevance: List[str]
    typical_roster_spots: int

class ScoringRule(BaseModel):
    """Definition of a fantasy football scoring rule."""
    
    name: str
    description: str
    points: float
    category: str
    applies_to: List[str]

class LeagueType(BaseModel):
    """Information about different types of fantasy leagues."""
    
    name: str
    description: str
    typical_settings: Dict[str, Any]
    common_strategies: List[str]

class FantasyFootballContext:
    """Provider of fantasy football domain context."""

    def __init__(self):
        """Initialize the fantasy football context provider."""
        self.positions = self._init_positions()
        self.scoring_rules = self._init_scoring_rules()
        self.league_types = self._init_league_types()

    def _init_positions(self) -> Dict[str, FantasyPosition]:
        """Initialize fantasy football position information.
        
        Returns:
            Dict[str, FantasyPosition]: Mapping of position codes to detailed information
        """
        return {
            "QB": FantasyPosition(
                code="QB",
                name="Quarterback",
                description="Team leader who throws passes and occasionally runs",
                scoring_relevance=["Passing yards", "Passing TDs", "Interceptions", "Rushing yards"],
                typical_roster_spots=1
            ),
            "RB": FantasyPosition(
                code="RB",
                name="Running Back",
                description="Primary ball carrier for rushing plays",
                scoring_relevance=["Rushing yards", "Rushing TDs", "Receptions", "Receiving yards"],
                typical_roster_spots=2
            ),
            "WR": FantasyPosition(
                code="WR",
                name="Wide Receiver",
                description="Primary pass catchers lined up wide",
                scoring_relevance=["Receptions", "Receiving yards", "Receiving TDs"],
                typical_roster_spots=3
            ),
            "TE": FantasyPosition(
                code="TE",
                name="Tight End",
                description="Hybrid position that blocks and catches passes",
                scoring_relevance=["Receptions", "Receiving yards", "Receiving TDs"],
                typical_roster_spots=1
            ),
            "K": FantasyPosition(
                code="K",
                name="Kicker",
                description="Specialist who kicks field goals and extra points",
                scoring_relevance=["Field goals", "Extra points"],
                typical_roster_spots=1
            ),
            "DEF": FantasyPosition(
                code="DEF",
                name="Team Defense/Special Teams",
                description="Entire defensive unit and special teams of an NFL team",
                scoring_relevance=["Sacks", "Interceptions", "Fumble recoveries", "Return TDs", "Points allowed"],
                typical_roster_spots=1
            ),
        }

    def _init_scoring_rules(self) -> Dict[str, ScoringRule]:
        """Initialize fantasy football scoring rules.
        
        Returns:
            Dict[str, ScoringRule]: Mapping of rule names to detailed information
        """
        return {
            "pass_td": ScoringRule(
                name="Passing Touchdown",
                description="Points awarded when a QB throws a touchdown pass",
                points=4.0,
                category="Passing",
                applies_to=["QB"]
            ),
            "rush_td": ScoringRule(
                name="Rushing Touchdown",
                description="Points awarded for running the ball into the endzone",
                points=6.0,
                category="Rushing",
                applies_to=["QB", "RB", "WR", "TE"]
            ),
            "rec_td": ScoringRule(
                name="Receiving Touchdown",
                description="Points awarded for catching a touchdown pass",
                points=6.0,
                category="Receiving",
                applies_to=["RB", "WR", "TE"]
            ),
            # Add more scoring rules as needed
        }

    def _init_league_types(self) -> Dict[str, LeagueType]:
        """Initialize fantasy football league types.
        
        Returns:
            Dict[str, LeagueType]: Mapping of league types to detailed information
        """
        return {
            "standard": LeagueType(
                name="Standard League",
                description="Traditional fantasy format with basic scoring",
                typical_settings={
                    "roster_size": 16,
                    "starters": {
                        "QB": 1,
                        "RB": 2,
                        "WR": 2,
                        "TE": 1,
                        "FLEX": 1,
                        "K": 1,
                        "DEF": 1
                    },
                    "ppr": 0.0
                },
                common_strategies=[
                    "Prioritize RBs early in draft",
                    "Wait on QB unless elite option available",
                    "Stream defenses based on matchups"
                ]
            ),
            "ppr": LeagueType(
                name="PPR League",
                description="Points Per Reception format that rewards catches",
                typical_settings={
                    "roster_size": 16,
                    "starters": {
                        "QB": 1,
                        "RB": 2,
                        "WR": 2,
                        "TE": 1,
                        "FLEX": 1,
                        "K": 1,
                        "DEF": 1
                    },
                    "ppr": 1.0
                },
                common_strategies=[
                    "Value receiving RBs higher",
                    "Target high-volume receivers",
                    "Consider pass-catching backs in FLEX"
                ]
            ),
            "dynasty": LeagueType(
                name="Dynasty League",
                description="Multi-year format where teams keep most/all players",
                typical_settings={
                    "roster_size": 25,
                    "starters": {
                        "QB": 1,
                        "RB": 2,
                        "WR": 3,
                        "TE": 1,
                        "FLEX": 2,
                        "SF": 1
                    },
                    "keeper_count": "All"
                },
                common_strategies=[
                    "Balance winning now vs future",
                    "Target young players with upside",
                    "Value draft picks for rebuilding"
                ]
            ),
        }

    def get_position_info(self, position_code: str) -> Optional[FantasyPosition]:
        """Get detailed information about a fantasy football position.
        
        Args:
            position_code: Position code (e.g., "QB", "RB")
            
        Returns:
            Optional[FantasyPosition]: Position information if found
        """
        return self.positions.get(position_code.upper())

    def get_scoring_rule(self, rule_name: str) -> Optional[ScoringRule]:
        """Get detailed information about a scoring rule.
        
        Args:
            rule_name: Name of the scoring rule
            
        Returns:
            Optional[ScoringRule]: Scoring rule information if found
        """
        return self.scoring_rules.get(rule_name.lower())

    def get_league_type(self, type_name: str) -> Optional[LeagueType]:
        """Get detailed information about a league type.
        
        Args:
            type_name: Name of the league type
            
        Returns:
            Optional[LeagueType]: League type information if found
        """
        return self.league_types.get(type_name.lower())

    def explain_scoring(self, stat_line: Dict[str, Any], position: str) -> str:
        """Explain how a stat line translates to fantasy points.
        
        Args:
            stat_line: Dictionary of statistics
            position: Player position
            
        Returns:
            str: Human-readable explanation of scoring
        """
        position = position.upper()
        if position not in self.positions:
            return "Unknown position"

        explanations = []
        total_points = 0.0

        # Example scoring calculations (customize based on your league settings)
        if position == "QB":
            if "passing_yards" in stat_line:
                points = stat_line["passing_yards"] * 0.04
                total_points += points
                explanations.append(f"Passing yards ({stat_line['passing_yards']}): {points:.1f} pts")
            
            if "passing_tds" in stat_line:
                points = stat_line["passing_tds"] * 4
                total_points += points
                explanations.append(f"Passing TDs ({stat_line['passing_tds']}): {points:.1f} pts")

        # Add more position-specific scoring explanations

        return "\n".join([
            f"Scoring explanation for {position}:",
            *explanations,
            f"Total points: {total_points:.1f}"
        ])

    def suggest_strategies(self, league_settings: Dict[str, Any]) -> List[str]:
        """Suggest fantasy football strategies based on league settings.
        
        Args:
            league_settings: Dictionary of league settings
            
        Returns:
            List[str]: List of strategy suggestions
        """
        strategies = []

        # Identify league type
        ppr = league_settings.get("scoring_settings", {}).get("reception", 0.0)
        keeper_count = league_settings.get("keeper_settings", {}).get("count", 0)

        # Add type-specific strategies
        if ppr > 0:
            strategies.extend([
                "Target high-volume receivers",
                "Value RBs with receiving work",
                "Consider pass-catching specialists in PPR FLEX spots"
            ])

        if keeper_count > 0:
            strategies.extend([
                "Consider player age and future potential",
                "Target young players with upside",
                "Monitor rookie draft picks"
            ])

        # Add general strategies
        strategies.extend([
            "Stay active on waiver wire",
            "Monitor injury reports",
            "Consider strength of schedule"
        ])

        return strategies
