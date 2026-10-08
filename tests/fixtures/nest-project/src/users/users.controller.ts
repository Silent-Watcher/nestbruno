import { Body, Controller, Delete, Get, Headers, Param, Patch, Post, Query } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll(@Query('page') page: number, @Query() filter: FilterDto) {}

  @Get(':id')
  findOne(@Param('id') id: string, @Query('include') include: string) {}

  @Post()
  create(@Body() dto: CreateUserDto) {}

  @Patch(':id')
  update(@Param('id') id: string, @Body('name') name: string, @Body('email') email: string) {}

  @Delete(':id')
  remove(@Param('id') id: string) {}
}
