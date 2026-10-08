@Controller('auth')
export class AuthController {
  @Post('login')
  login(@Body() creds: LoginDto, @Headers('x-trace-id') trace: string) {}

  @Head('health')
  health() {}

  @Options()
  options() {}

  @Get(':id?')
  maybe() {}

  @All('any')
  any() {}
}
