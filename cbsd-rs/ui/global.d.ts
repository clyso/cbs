
import 'vue-router';

declare module 'vue-router' {
  interface RouteMeta {
    isPublic?: () => boolean;
    isAuth?: () => boolean;
    isAuthorized?: () => boolean;
  }
}
