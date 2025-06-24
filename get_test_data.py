import asyncio
import aiohttp

async def get_test_data():
    async with aiohttp.ClientSession() as session:
        # Get a known user
        async with session.get('https://api.sleeper.app/v1/user/SleepierBot') as resp:
            user = await resp.json()
            print(f"TEST_USER_ID = {user['user_id']!r}")
            
            # Get their leagues
            async with session.get(f'https://api.sleeper.app/v1/user/{user["user_id"]}/leagues/nfl/2023') as resp:
                leagues = await resp.json()
                if leagues:
                    print(f"TEST_LEAGUE_ID = {leagues[0]['league_id']!r}")

asyncio.run(get_test_data())
