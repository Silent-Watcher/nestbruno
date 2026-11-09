import { NestFactory } from '@nestjs/core';
import { Controller, Get, Post, Version, VERSION_NEUTRAL } from '@nestjs/common';
import { VersioningType } from '@nestjs/common';
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  app.enableVersioning({ type: VersioningType.URI, defaultVersion: '1' });
}
bootstrap();
